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
import dev.phonelink.core.BatteryAlertKind
import dev.phonelink.core.CallPhaseFfi
import dev.phonelink.core.Device
import dev.phonelink.core.EventListener
import dev.phonelink.core.LinkEvent
import dev.phonelink.core.MirroredNotification
import dev.phonelink.core.PhoneMedia
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

    private val _messages = MutableStateFlow<List<dev.phonelink.app.ui.Note>>(emptyList())
    val messages: StateFlow<List<dev.phonelink.app.ui.Note>> = _messages

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
        _connectionOn.value = Prefs.connection(app)
        FileShare.init(app)
        SmsSync.init(app)
        BluetoothBackup.init(app)
    }

    private fun dataDir(context: Context) = File(context.filesDir, "phonelink")

    /** Starts the core on first use. Blocking: call from Dispatchers.IO. */
    @Synchronized
    fun link(): PhoneLink {
        link?.let { return it }
        val dir = dataDir(app).apply { mkdirs() }
        val started = PhoneLink.start(dir.path, deviceName(), listener, FileShare)
        started.setQueueNotifications(Prefs.queueNotifications(app))
        started.setPaused(!Prefs.connection(app))
        link = started
        _myId.value = started.id()
        refresh()
        io.execute { runCatching { FileShare.publishLeftovers(started.transfers()) } }
        return started
    }

    // ------------------------------------------------------------------ SMS

    /** All conversations, for computers that just connected (or after access was granted). */
    fun sendSmsSnapshot() = onIo { if (SmsSync.canRead()) it.smsSnapshot(SmsSync.snapshot()) }

    /** The SMS store changed: send the conversations that changed. */
    fun smsChanged() = onIo { link -> SmsSync.changedThreads().forEach { link.smsThreadUpdate(it) } }

    private fun smsSend(device: String, clientId: String, address: String, body: String) {
        SmsSync.send(address, body) { ok, message ->
            onIo { it.smsSendResult(device, clientId, ok, message) }
        }
    }

    private fun smsOlder(device: String, thread: Long, beforeMs: Long) = onIo {
        val (messages, more) = SmsSync.older(thread, beforeMs)
        it.smsOlder(device, thread.toULong(), messages, more)
    }

    /** Send files to the computer (from the Share menu, or picked in the app). */
    fun sendFiles(uris: List<android.net.Uri>, fromShare: Boolean) {
        io.execute {
            val msg = try {
                FileShare.send(link(), uris, fromShare)
            } catch (e: Exception) {
                e.message ?: "Couldn't send"
            }
            toast(msg)
            refresh()
        }
    }

    fun cancelTransfer(id: String) = onIo { it.cancelTransfer(id) }

    fun clearTransfers() = onIo { it.clearTransfers(); refresh() }

    private val _connectionOn = MutableStateFlow(true)
    /** The connection switch, for the UI. */
    val connectionOn: StateFlow<Boolean> = _connectionOn

    /**
     * Switch the connection on or off. Off: the computer is told and stops searching, the
     * background service stops (no notification, nothing running), Bluetooth isn't tried.
     */
    fun setConnection(on: Boolean) {
        Prefs.setConnection(app, on)
        _connectionOn.value = on
        io.execute {
            runCatching { link().setPaused(!on) }
            refresh()
        }
        val service = android.content.Intent(app, LinkService::class.java)
        if (on) androidx.core.content.ContextCompat.startForegroundService(app, service) else app.stopService(service)
    }

    fun setQueueNotifications(on: Boolean) {
        Prefs.setQueueNotifications(app, on)
        onIo { it.setQueueNotifications(on) }
    }

    /** What plays on the computer and its own buttons, for the Remote screen. */
    private val _computerMedia = MutableStateFlow<PhoneMedia?>(null)
    val computerMedia: StateFlow<PhoneMedia?> = _computerMedia
    /** The computer's album art: (key, picture). Matches `computerMedia.artKey` when current. */
    private val _computerArt = MutableStateFlow<Pair<String, android.graphics.Bitmap>?>(null)
    val computerArt: StateFlow<Pair<String, android.graphics.Bitmap>?> = _computerArt
    /** When something last played on the computer (ms), so a paused player hides after a while. */
    private val _computerPlayedMs = MutableStateFlow(0L)
    val computerPlayedMs: StateFlow<Long> = _computerPlayedMs
    private val _commands = MutableStateFlow<List<dev.phonelink.core.RemoteButton>>(emptyList())
    val commands: StateFlow<List<dev.phonelink.core.RemoteButton>> = _commands

    /** A remote button press: says nothing when it works, a toast when it can't be sent. */
    private fun remote(block: (PhoneLink) -> Unit) = io.execute {
        try {
            block(link())
        } catch (e: Exception) {
            toast(e.message?.replaceFirstChar { it.uppercase() } ?: "Not connected")
        }
    }

    fun computerMedia(command: dev.phonelink.core.MediaCommand) = remote { it.computerMediaControl(null, command) }
    fun runCommand(id: String) = remote { it.runCommand(null, id) }

    /** The presentation pointer (sent only while connected; never queued). */
    fun pointer(x: Int, y: Int, visible: Boolean, shape: String, color: Int, size: Int) = io.execute {
        val s = when (shape) {
            "ring" -> dev.phonelink.core.PointerShapeFfi.RING
            "spotlight" -> dev.phonelink.core.PointerShapeFfi.SPOTLIGHT
            else -> dev.phonelink.core.PointerShapeFfi.DOT
        }
        runCatching { link().pointer(null, x.toUShort(), y.toUShort(), visible, s, color.toUInt(), size.toUByte()) }
    }
    fun presentKey(key: dev.phonelink.core.PresentKeyFfi) = remote { it.presentKey(null, key) }

    private val _transfers = MutableStateFlow<List<dev.phonelink.core.FileTransfer>>(emptyList())
    val transfers: StateFlow<List<dev.phonelink.core.FileTransfer>> = _transfers

    fun refresh() {
        link?.let {
            _devices.value = it.devices()
            _transfers.value = it.transfers().take(20)
        }
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
        if (!Prefs.connection(app)) return // switched off: nothing is sent
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
     * Run a "right now" request. If EchoTwin was just started (from a tile or the
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

    /** Unlock the computer (after the fingerprint prompt). Its answer arrives as a result. */
    fun unlockComputer() = whenConnected { link ->
        val name = link.unlockComputer(null)
        "Unlocking $name…"
    }

    /** Ring the computer (to find it), or stop it. */
    fun ringComputer(on: Boolean) = whenConnected { link ->
        val name = link.ringComputer(null, on)
        if (on) "Ringing $name…" else "Stopped"
    }

    /** Share → "Open on computer": the first web link in `text`, in the computer's browser. */
    fun openOnComputer(text: String) {
        val url = Regex("""https?://\S+""", RegexOption.IGNORE_CASE).find(text)?.value
        if (url == null) {
            toast("No web link to open")
            return
        }
        whenConnected { link -> "Opening on ${link.openUrl(null, url)}…" }
    }

    /** Put `text` on the computer's clipboard. */
    fun sendClipboard(text: String) = whenConnected { link ->
        if (!Prefs.clipboardSharing(app)) throw IllegalStateException("Clipboard sharing is off in EchoTwin")
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

    /** Has the user granted "Notification access" to EchoTwin? */
    fun mirrorEnabled(context: Context): Boolean =
        NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)

    fun mirrorPosted(n: MirroredNotification) = onIo { it.postNotification(n) }
    fun mirrorRemoved(key: String) = onIo { it.removeNotification(key) }
    fun mirrorIcon(pkg: String, png: ByteArray) = onIo { it.sendAppIcon(pkg, png) }

    /** What's playing, for connected computers (not queued while they're offline). */
    fun sendNowPlaying(media: PhoneMedia?) = onIo { it.sendNowPlaying(media) }
    fun sendMediaArt(key: String, image: ByteArray) = onIo { it.sendMediaArt(key, image) }

    /** A call started, changed or ended (connected computers only). */
    fun sendCallInfo(phase: CallPhaseFfi, caller: String, key: String, actions: List<dev.phonelink.core.CallActionFfi>) =
        onIo { it.sendCallInfo(phase, caller, key, actions) }

    /** Battery and network for connected computers (not queued). */
    fun sendPhoneStatus(battery: Int?, charging: Boolean, network: String, signal: Int) = onIo {
        it.sendPhoneStatus(battery?.toUByte(), charging, network, signal.toUByte())
    }

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
                    _messages.value = (listOf(dev.phonelink.app.ui.Note(event.name, event.text)) + _messages.value).take(50)
                    Notifications.message(app, event.name, event.text, copy = true)
                }
                is LinkEvent.Disconnected -> {
                    BluetoothBackup.onDisconnected()
                    _computerMedia.value = null
                }
                is LinkEvent.ComputerMedia -> {
                    _computerMedia.value = event.media
                    if (event.media?.playing == true) _computerPlayedMs.value = System.currentTimeMillis()
                }
                is LinkEvent.ComputerMediaArt -> {
                    val bmp = android.graphics.BitmapFactory.decodeByteArray(event.image, 0, event.image.size)
                    if (bmp != null) _computerArt.value = event.key to bmp
                }
                is LinkEvent.ContactsRequested -> io.execute {
                    runCatching { link().contactsResult(event.id, event.req, SmsSync.searchContacts(event.query)) }
                }
                is LinkEvent.RemoteCommands -> _commands.value = event.items
                is LinkEvent.Connected -> {
                    BluetoothBackup.onConnected()
                    sendSnapshot()
                    MediaWatcher.resend()
                    StatusWatcher.send(force = true)
                    CallWatcher.resend()
                    sendSmsSnapshot()
                }
                is LinkEvent.SmsSendRequested -> smsSend(event.id, event.clientId, event.address, event.body)
                is LinkEvent.SmsOlderRequested -> smsOlder(event.id, event.thread.toLong(), event.beforeMs.toLong())
                is LinkEvent.MediaControlRequested -> MediaWatcher.control(event.command)
                is LinkEvent.DismissRequested -> handleRequest(event.id, event.key) { it.dismiss(event.key) }
                is LinkEvent.ReplyRequested -> handleRequest(event.id, event.key) { it.reply(event.key, event.text) }
                is LinkEvent.ActionRequested -> handleRequest(event.id, event.key) { it.press(event.key, event.index.toInt()) }
                is LinkEvent.ClipboardReceived -> putOnClipboard(event.name, event.text)
                is LinkEvent.ClipboardImageIncoming -> FileShare.expectClipboard(event.transferId, event.name)
                is LinkEvent.TransferChanged -> {
                    val all = { link?.transfers() ?: emptyList() }
                    io.execute {
                        FileShare.onChanged(event.transfer, all)
                        _transfers.value = all().take(20)
                    }
                }
                is LinkEvent.OpenUrlRequested -> Notifications.link(app, event.name, event.url)
                is LinkEvent.CallDeclineRequested -> handleRequest(event.id, "call") { CallWatcher.decline(it, event.key) }
                is LinkEvent.CallActionRequested -> handleRequest(event.id, "call") { CallWatcher.act(it, event.key, event.action) }
                is LinkEvent.RingRequested -> if (event.on) Ringer.start(app, event.name) else Ringer.stop()
                is LinkEvent.ComputerStatus -> event.alert?.let { alert ->
                    if (Prefs.batteryAlerts(app)) {
                        val text = when (alert) {
                            BatteryAlertKind.LOW -> "Battery low (${event.battery ?: 0u}%). Plug it in soon."
                            BatteryAlertKind.FULL -> "Fully charged. You can unplug it."
                        }
                        Notifications.message(app, event.name, text)
                    }
                }
                is LinkEvent.RequestResult -> when {
                    event.key == "lock" -> toast(if (event.ok) "${event.name} is locked" else "${event.name}: ${event.message}")
                    event.key.startsWith("cmd:") -> toast(if (event.ok) event.message else "${event.name}: ${event.message}")
                    !event.ok && (event.key == "media" || event.key == "present") -> toast("${event.name}: ${event.message}")
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
