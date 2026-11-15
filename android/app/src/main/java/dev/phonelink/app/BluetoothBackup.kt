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
 * The Bluetooth backup: when the computer is unreachable over Wi-Fi, connect to it over
 * Bluetooth (RFCOMM) and run the same encrypted session there. When Wi-Fi comes back, the
 * core switches over and closes the Bluetooth socket.
 *
 * Takes over after 15 s if the two are paired in the phone's Bluetooth settings, and after
 * 2 minutes if not: then it opens an *insecure* RFCOMM socket (no Bluetooth pairing, no
 * popup). That is safe because the session on top is mutual TLS with pinned keys, the same
 * as over Wi-Fi; Bluetooth's own pairing adds nothing we rely on.
 *
 * Only connects to the computer you paired EchoTwin with (its Bluetooth address arrives
 * over the encrypted link). Needs "Nearby devices" (BLUETOOTH_CONNECT) on Android 12+;
 * never scans, never uses location or audio.
 */
object BluetoothBackup {
    /** Must match the laptop (desktop/daemon/src/bluetooth.rs PROFILE_UUID / CHANNELS). */
    private val SERVICE: UUID = UUID.fromString("7a1d5c3e-9b4f-4e8a-a2c6-5f0e3d1b8c94")
    private val CHANNELS = intArrayOf(22, 23, 24)
    private const val FIRST_TRY_MS = 15_000L
    private const val MAX_WAIT_MS = 120_000L
    /** Without Bluetooth pairing, wait this long without Wi-Fi before connecting. */
    const val UNPAIRED_AFTER_MS = 120_000L

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "echotwin-bluetooth") }
    private var delay = FIRST_TRY_MS
    @Volatile private var socket: BluetoothSocket? = null
    /** When the computer stopped being reachable (0 = reachable). Main thread. */
    @Volatile private var unreachableSince = 0L
    /** The laptop is paired in Bluetooth settings (null = unknown yet). For the Settings text. */
    @Volatile var bonded: Boolean? = null
        private set

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
    fun start() {
        markUnreachable()
        schedule(FIRST_TRY_MS)
    }

    /** The computer dropped off: try Bluetooth if it doesn't come back over Wi-Fi soon. */
    fun onDisconnected() {
        markUnreachable()
        schedule(FIRST_TRY_MS)
    }

    private fun markUnreachable() = main.post {
        if (unreachableSince == 0L) unreachableSince = System.currentTimeMillis()
    }

    /** The network changed: if we're still not connected, look again soon. */
    fun onNetworkChanged() = schedule(FIRST_TRY_MS)

    /** Connected again (over either link): no more attempts for now. */
    fun onConnected() = main.post {
        main.removeCallbacks(attempt)
        delay = FIRST_TRY_MS
        unreachableSince = 0L
    }

    private fun schedule(ms: Long) = main.post {
        if (!::app.isInitialized || !Prefs.connection(app)) return@post
        main.removeCallbacks(attempt)
        delay = ms
        main.postDelayed(attempt, ms)
    }

    /** Set by [target] when it is only waiting out the 2 minutes: try again right then. */
    @Volatile private var waitMs = 0L

    private fun retryLater() {
        val wait = waitMs
        waitMs = 0L
        main.post {
            if (!Prefs.connection(app)) return@post // switched off: stop trying
            main.removeCallbacks(attempt)
            if (wait > 0) {
                main.postDelayed(attempt, wait + 500)
            } else {
                delay = (delay * 2).coerceAtMost(MAX_WAIT_MS)
                main.postDelayed(attempt, delay)
            }
        }
    }

    /**
     * The paired computer and its Bluetooth device, or why not (also sets [problem]).
     * `waitForUnpaired`: without Bluetooth pairing, only after [UNPAIRED_AFTER_MS] without
     * Wi-Fi (the Settings test skips that wait).
     */
    @SuppressLint("MissingPermission") // checked in allowed()
    private fun target(link: PhoneLink, waitForUnpaired: Boolean = true): Pair<Device, BluetoothDevice>? {
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
        val isBonded = device.bondState == BluetoothDevice.BOND_BONDED
        bonded = isBonded
        if (!isBonded && waitForUnpaired) {
            val since = unreachableSince
            val left = if (since == 0L) UNPAIRED_AFTER_MS else UNPAIRED_AFTER_MS - (System.currentTimeMillis() - since)
            if (left > 0) {
                problem = "Not paired in Bluetooth settings: takes over 2 min after Wi-Fi drops"
                waitMs = left
                return null
            }
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
        // Not paired in Bluetooth settings: an insecure link, so no pairing popup appears.
        // (The TLS session on top authenticates and encrypts; never call createBond().)
        val secure = device.bondState == BluetoothDevice.BOND_BONDED
        var last: Exception? = null
        try {
            val s = if (secure) device.createRfcommSocketToServiceRecord(SERVICE)
            else device.createInsecureRfcommSocketToServiceRecord(SERVICE)
            return s.also { it.connect() }
        } catch (e: Exception) {
            last = e
        }
        val method = if (secure) "createRfcommSocket" else "createInsecureRfcommSocket"
        for (ch in CHANNELS) {
            try {
                val s = device.javaClass.getMethod(method, Int::class.javaPrimitiveType)
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
            val (computer, device) = target(link, waitForUnpaired = false) ?: return@run problem ?: "Not ready"
            try {
                open(device).close()
                problem = null
                if (device.bondState == BluetoothDevice.BOND_BONDED) {
                    "${computer.name} is reachable over Bluetooth ✓ It takes over 15 s after Wi-Fi drops."
                } else {
                    "${computer.name} is reachable over Bluetooth without pairing ✓ It takes over 2 min " +
                        "after Wi-Fi drops (pair them in Bluetooth settings for 15 s)."
                }
            } catch (e: Exception) {
                "${computer.name} didn't answer over Bluetooth (${e.javaClass.simpleName}). Check that " +
                    "Bluetooth backup is on in EchoTwin on the computer and that `phonelink status` says ready."
            }
        }
        main.post { report(result) }
    }
}
