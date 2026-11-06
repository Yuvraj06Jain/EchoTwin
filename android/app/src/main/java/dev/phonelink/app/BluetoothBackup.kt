package dev.phonelink.app

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import dev.phonelink.core.StreamSink
import java.util.UUID
import java.util.concurrent.Executors

/**
 * The Bluetooth backup: when the computer has been unreachable over Wi-Fi for 15 seconds,
 * connect to it over Bluetooth (RFCOMM) and run the same encrypted session there. When
 * Wi-Fi comes back, the core switches over and closes the Bluetooth socket.
 *
 * Only connects to the computer you paired with (its Bluetooth address arrives over the
 * encrypted link), and only if it is already paired in the phone's Bluetooth settings.
 * Needs "Nearby devices" (BLUETOOTH_CONNECT) on Android 12+; never scans, never uses
 * location or audio.
 */
object BluetoothBackup {
    /** Must match the laptop (desktop/daemon/src/bluetooth.rs). */
    private val SERVICE: UUID = UUID.fromString("7a1d5c3e-9b4f-4e8a-a2c6-5f0e3d1b8c94")
    private const val FIRST_TRY_MS = 15_000L
    private const val MAX_WAIT_MS = 120_000L

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "phonelink-bluetooth") }
    private var delay = FIRST_TRY_MS
    @Volatile private var socket: BluetoothSocket? = null

    /** Why the backup isn't usable right now (shown in the app), or null. */
    @Volatile var problem: String? = null
        private set

    fun init(context: Context) {
        app = context.applicationContext
    }

    fun allowed(): Boolean = Build.VERSION.SDK_INT < 31 ||
        ContextCompat.checkSelfPermission(app, Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

    val permission = Manifest.permission.BLUETOOTH_CONNECT

    private val attempt = Runnable { worker.execute { tryConnect() } }

    /** The computer dropped off: try Bluetooth if it doesn't come back over Wi-Fi soon. */
    fun onDisconnected() = main.post {
        main.removeCallbacks(attempt)
        delay = FIRST_TRY_MS
        main.postDelayed(attempt, delay)
    }

    /** Connected again (over either link): no more attempts for now. */
    fun onConnected() = main.post {
        main.removeCallbacks(attempt)
        delay = FIRST_TRY_MS
    }

    private fun retryLater() {
        main.post {
            delay = (delay * 2).coerceAtMost(MAX_WAIT_MS)
            main.removeCallbacks(attempt)
            main.postDelayed(attempt, delay)
        }
    }

    @SuppressLint("MissingPermission") // checked in allowed()
    private fun tryConnect() {
        val link = try {
            LinkManager.link()
        } catch (e: Exception) {
            return
        }
        val computer = link.devices().firstOrNull() ?: return
        if (computer.connected) return
        val address = computer.bluetooth
        if (address.isNullOrBlank()) {
            problem = "The computer has the Bluetooth backup off, or no Bluetooth"
            return retryLater()
        }
        if (!allowed()) {
            problem = "Allow \"Nearby devices\" in the PhoneLink app"
            return
        }
        val adapter = app.getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter == null || !adapter.isEnabled) {
            problem = "Bluetooth is off on this phone"
            return retryLater()
        }
        val device = try {
            adapter.getRemoteDevice(address)
        } catch (e: IllegalArgumentException) {
            return
        }
        if (device.bondState != BluetoothDevice.BOND_BONDED) {
            // Connecting would pop up a pairing request: leave that to the user.
            problem = "Pair this phone with ${computer.name} in Bluetooth settings first"
            return retryLater()
        }
        val sock = try {
            device.createRfcommSocketToServiceRecord(SERVICE).also { it.connect() }
        } catch (e: Exception) {
            problem = "${computer.name} didn't answer over Bluetooth"
            return retryLater()
        }
        problem = null
        socket = sock
        val pipe = try {
            link.openStream(computer.id, object : StreamSink {
                override fun write(data: ByteArray): Boolean = try {
                    sock.outputStream.write(data)
                    sock.outputStream.flush()
                    true
                } catch (e: Exception) {
                    false
                }

                override fun closeLink() {
                    runCatching { sock.close() }
                }
            })
        } catch (e: Exception) {
            runCatching { sock.close() }
            return // Wi-Fi came back meanwhile
        }
        // Read until the socket closes (out of range, or the core closed it for Wi-Fi).
        Thread({
            val buf = ByteArray(16 * 1024)
            try {
                while (true) {
                    val n = sock.inputStream.read(buf)
                    if (n < 0 || !pipe.received(buf.copyOf(n))) break
                }
            } catch (e: Exception) {
                // closed
            } finally {
                pipe.closed()
                runCatching { sock.close() }
                if (socket === sock) socket = null
            }
        }, "phonelink-bluetooth-read").start()
    }
}
