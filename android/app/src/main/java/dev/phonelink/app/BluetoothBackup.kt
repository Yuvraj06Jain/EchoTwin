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
import dev.phonelink.core.Device
import dev.phonelink.core.PhoneLink
import dev.phonelink.core.StreamSink
import java.util.UUID
import java.util.concurrent.Executors

/**
 * The Bluetooth backup: whenever the computer has been unreachable for 15 seconds, connect to
 * it over Bluetooth (RFCOMM) and run the same encrypted session there. When Wi-Fi comes
 * back, the core switches over and closes the Bluetooth socket.
 *
 * Only connects to the computer you paired with (its Bluetooth address arrives over the
 * encrypted link), and only if it is already paired in the phone's Bluetooth settings.
 * Needs "Nearby devices" (BLUETOOTH_CONNECT) on Android 12+; never scans, never uses
 * location or audio.
 */
object BluetoothBackup {
    /** Must match the laptop (desktop/daemon/src/bluetooth.rs PROFILE_UUID / CHANNELS). */
    private val SERVICE: UUID = UUID.fromString("7a1d5c3e-9b4f-4e8a-a2c6-5f0e3d1b8c94")
    private val CHANNELS = intArrayOf(22, 23, 24)
    private const val FIRST_TRY_MS = 15_000L
    private const val MAX_WAIT_MS = 120_000L

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "echotwin-bluetooth") }
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

    /** Connected over Bluetooth right now. */
    val active: Boolean get() = socket != null

    private val attempt = Runnable { worker.execute { tryConnect() } }

    /**
     * Start watching (the background service started). Also covers starting while the
     * computer is already unreachable, when no "disconnected" event will ever come.
     */
    fun start() = schedule(FIRST_TRY_MS)

    /** The computer dropped off: try Bluetooth if it doesn't come back over Wi-Fi soon. */
    fun onDisconnected() = schedule(FIRST_TRY_MS)

    /** The network changed: if we're still not connected, look again soon. */
    fun onNetworkChanged() = schedule(FIRST_TRY_MS)

    /** Connected again (over either link): no more attempts for now. */
    fun onConnected() = main.post {
        main.removeCallbacks(attempt)
        delay = FIRST_TRY_MS
    }

    private fun schedule(ms: Long) = main.post {
        if (!::app.isInitialized || !Prefs.connection(app)) return@post
        main.removeCallbacks(attempt)
        delay = ms
        main.postDelayed(attempt, ms)
    }

    private fun retryLater() {
        main.post {
            if (!Prefs.connection(app)) return@post // switched off: stop trying
            delay = (delay * 2).coerceAtMost(MAX_WAIT_MS)
            main.removeCallbacks(attempt)
            main.postDelayed(attempt, delay)
        }
    }

    /** The paired computer and its bonded Bluetooth device, or why not (also sets [problem]). */
    @SuppressLint("MissingPermission") // checked in allowed()
    private fun target(link: PhoneLink): Pair<Device, BluetoothDevice>? {
        val computer = link.devices().firstOrNull()
        if (computer == null) {
            problem = "No computer paired"
            return null
        }
        val address = computer.bluetooth
        if (address.isNullOrBlank()) {
            problem = "${computer.name} hasn't shared its Bluetooth address yet: turn on its Bluetooth " +
                "backup (EchoTwin Settings) and connect once over Wi-Fi"
            return null
        }
        if (!allowed()) {
            problem = "Allow \"Nearby devices\" in the EchoTwin app"
            return null
        }
        val adapter = app.getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter == null || !adapter.isEnabled) {
            problem = "Bluetooth is off on this phone"
            return null
        }
        val device = try {
            adapter.getRemoteDevice(address)
        } catch (e: IllegalArgumentException) {
            problem = "${computer.name} sent an invalid Bluetooth address"
            return null
        }
        if (device.bondState != BluetoothDevice.BOND_BONDED) {
            // Connecting would pop up a pairing request: leave that to the user.
            problem = "Pair this phone with ${computer.name} in Bluetooth settings first"
            return null
        }
        return computer to device
    }

    /**
     * An RFCOMM socket to the computer: by service lookup first, then straight to the
     * channels the laptop listens on (some phones' lookups miss a freshly added service).
     */
    @SuppressLint("MissingPermission")
    private fun open(device: BluetoothDevice): BluetoothSocket {
        app.getSystemService(BluetoothManager::class.java)?.adapter?.cancelDiscovery()
        var last: Exception? = null
        try {
            return device.createRfcommSocketToServiceRecord(SERVICE).also { it.connect() }
        } catch (e: Exception) {
            last = e
        }
        for (ch in CHANNELS) {
            try {
                val s = device.javaClass.getMethod("createRfcommSocket", Int::class.javaPrimitiveType)
                    .invoke(device, ch) as BluetoothSocket
                s.connect()
                return s
            } catch (e: Exception) {
                last = e
            }
        }
        throw last ?: IllegalStateException("no socket")
    }

    private fun tryConnect() {
        if (!Prefs.connection(app)) return
        val link = try {
            LinkManager.link()
        } catch (e: Exception) {
            return retryLater()
        }
        if (link.devices().firstOrNull()?.connected == true) {
            return // over Wi-Fi again: nothing to do until it drops
        }
        val (computer, device) = target(link) ?: return retryLater()
        val sock = try {
            open(device)
        } catch (e: Exception) {
            problem = "${computer.name} didn't answer over Bluetooth. Is EchoTwin running there " +
                "with Bluetooth backup on, and Bluetooth switched on?"
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
            socket = null
            return retryLater() // Wi-Fi came back meanwhile, or the session was refused
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
                schedule(FIRST_TRY_MS) // still no Wi-Fi? try again
            }
        }, "echotwin-bluetooth-read").start()
    }

    /**
     * "Test" in Settings: every check, then a real Bluetooth connection that is closed
     * again at once. Reports the result in plain words on the main thread.
     */
    fun test(report: (String) -> Unit) = worker.execute {
        val result = run {
            if (active) return@run "Connected over Bluetooth right now ✓"
            val link = try {
                LinkManager.link()
            } catch (e: Exception) {
                return@run "EchoTwin isn't ready yet; try again in a moment"
            }
            val (computer, device) = target(link) ?: return@run problem ?: "Not ready"
            try {
                open(device).close()
                problem = null
                "${computer.name} is reachable over Bluetooth ✓ It takes over when Wi-Fi is gone."
            } catch (e: Exception) {
                "${computer.name} didn't answer over Bluetooth (${e.javaClass.simpleName}). Check that " +
                    "Bluetooth backup is on in EchoTwin on the computer and that `phonelink status` says ready."
            }
        }
        main.post { report(result) }
    }
}
