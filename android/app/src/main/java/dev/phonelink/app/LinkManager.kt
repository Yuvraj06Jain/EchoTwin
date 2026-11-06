package dev.phonelink.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.provider.Settings
import android.widget.Toast
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

    /** Bumped when a phone-side switch changes, so the status notification updates. */
    private val _settingsVersion = MutableStateFlow(0)
    val settingsVersion: StateFlow<Int> = _settingsVersion

    fun settingsChanged() {
        _settingsVersion.value += 1
    }

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

    private val main = Handler(Looper.getMainLooper())

    /** A short message on screen, from any thread (also from the background service). */
    fun toast(text: String) {
        main.post { Toast.makeText(app, text, Toast.LENGTH_SHORT).show() }
    }

    /**
     * Run a "right now" request. If PhoneLink was just started (from a tile or the
     * notification), give the connection a few seconds to come up before giving up.
     */
    private fun whenConnected(block: (PhoneLink) -> String) {
        io.execute {
            val message = try {
                val link = link()
                var result: Result<String> = runCatching { block(link) }
                val deadline = System.currentTimeMillis() + 6_000
                while (result.isFailure && System.currentTimeMillis() < deadline &&
                    link.devices().none { it.connected }
                ) {
                    link.onNetworkChanged()
                    Thread.sleep(300)
                    if (link.devices().any { it.connected }) result = runCatching { block(link) }
                }
                refresh()
                result.fold({ it }, { it.message ?: "Failed" })
            } catch (e: Exception) {
                e.message ?: "Failed"
            }
            toast(message.replaceFirstChar { it.uppercase() })
        }
    }

    /** Lock the computer's screen (tile, notification button, app). */
    fun lockComputer() = whenConnected { link ->
        val name = link.lockComputer(null)
        "Locking $name…"
    }

    /** Put `text` on the computer's clipboard. */
    fun sendClipboard(text: String) = whenConnected { link ->
        if (!Prefs.clipboardSharing(app)) throw IllegalStateException("Clipboard sharing is off in PhoneLink")
        link.sendClipboard(text)
        "Copied to your computer"
    }

    private fun putOnClipboard(from: String, text: String) {
        if (!Prefs.clipboardSharing(app)) return
        main.post {
            val clip = ClipData.newPlainText("From $from", text)
            // Keep it out of Android's clipboard preview pop-up (it may be a password).
            clip.description.extras = PersistableBundle().apply {
                putBoolean("android.content.extra.IS_SENSITIVE", true)
            }
            runCatching { app.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip) }
        }
    }

    /** Try to reach the computer right now (skip the back-off, search the network). */
    fun nudge() = onIo { it.onNetworkChanged() }

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
                is LinkEvent.ClipboardReceived -> putOnClipboard(event.name, event.text)
                is LinkEvent.RequestResult -> if (event.key == "lock") {
                    toast(if (event.ok) "${event.name} is locked" else "${event.name}: ${event.message}")
                }
                else -> {}
            }
        }
    }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
}
