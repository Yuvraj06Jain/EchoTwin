package dev.phonelink.app

import android.content.Context
import android.os.Build
import android.provider.Settings
import dev.phonelink.core.Device
import dev.phonelink.core.EventListener
import dev.phonelink.core.LinkEvent
import dev.phonelink.core.MirroredNotification
import dev.phonelink.core.PhoneLink
import androidx.core.app.NotificationManagerCompat
import java.util.concurrent.Executors
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/**
 * Process-wide owner of the Rust core. The foreground service keeps the process (and so
 * this object) alive; the UI only observes the flows.
 */
object LinkManager {
    private lateinit var app: Context

    @Volatile
    private var link: PhoneLink? = null

    private val _devices = MutableStateFlow<List<Device>>(emptyList())
    val devices: StateFlow<List<Device>> = _devices

    private val _messages = MutableStateFlow<List<String>>(emptyList())
    val messages: StateFlow<List<String>> = _messages

    private val _myId = MutableStateFlow("")
    val myId: StateFlow<String> = _myId

    fun init(context: Context) {
        app = context.applicationContext
    }

    private fun dataDir(context: Context) = File(context.filesDir, "phonelink")

    /** Starts the core on first use. Blocking: call from Dispatchers.IO. */
    @Synchronized
    fun link(): PhoneLink {
        link?.let { return it }
        val dir = dataDir(app).apply { mkdirs() }
        val started = PhoneLink.start(dir.path, deviceName(), listener)
        link = started
        _myId.value = started.id()
        refresh()
        return started
    }

    fun refresh() {
        link?.let { _devices.value = it.devices() }
    }

    /** Cheap check usable from a BroadcastReceiver (no core start-up). */
    fun hasPairedDevice(context: Context): Boolean {
        val peers = File(dataDir(context), "peers.json")
        return peers.exists() && peers.readText().contains("\"fingerprint\"")
    }

    // ------------------------------------------------------------------ notification mirroring

    /** Disk + network work for mirroring, off the listener's main thread, in order. */
    private val io = Executors.newSingleThreadExecutor { r -> Thread(r, "phonelink-mirror") }

    private fun onIo(block: (PhoneLink) -> Unit) {
        io.execute {
            try {
                block(link())
            } catch (e: Exception) {
                // Not paired yet, or the core is shutting down: mirroring resumes on reconnect.
            }
        }
    }

    /** Has the user granted "Notification access" to PhoneLink? */
    fun mirrorEnabled(context: Context): Boolean =
        NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)

    fun mirrorPosted(n: MirroredNotification) = onIo { it.postNotification(n) }
    fun mirrorRemoved(key: String) = onIo { it.removeNotification(key) }
    fun mirrorIcon(pkg: String, png: ByteArray) = onIo { it.sendAppIcon(pkg, png) }

    /** The listener (re)connected: give the computer the full picture. */
    fun onMirrorReady() = sendSnapshot()

    private fun sendSnapshot() {
        val service = MirrorService.instance ?: return
        onIo { it.sendSnapshot(service.snapshot()) }
    }

    /** A request from the computer about one of the notifications this phone sent. */
    private fun handleRequest(device: String, key: String, action: (MirrorService) -> Result<String>) {
        io.execute {
            val service = MirrorService.instance
            val result = service?.let(action) ?: Result.failure(IllegalStateException("Notification access is off on the phone"))
            try {
                link().sendActionResult(device, key, result.isSuccess, result.fold({ it }, { it.message ?: "Failed" }))
            } catch (e: Exception) {
                // The computer will see no result; nothing else to do.
            }
        }
    }

    private val listener = object : EventListener {
        override fun onEvent(event: LinkEvent) {
            refresh()
            when (event) {
                is LinkEvent.Text -> {
                    _messages.value = (listOf("${event.name}: ${event.text}") + _messages.value).take(50)
                    Notifications.message(app, event.name, event.text)
                }
                is LinkEvent.Connected -> sendSnapshot()
                is LinkEvent.DismissRequested -> handleRequest(event.id, event.key) { it.dismiss(event.key) }
                is LinkEvent.ReplyRequested -> handleRequest(event.id, event.key) { it.reply(event.key, event.text) }
                is LinkEvent.ActionRequested -> handleRequest(event.id, event.key) { it.press(event.key, event.index.toInt()) }
                else -> {}
            }
        }
    }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
}
