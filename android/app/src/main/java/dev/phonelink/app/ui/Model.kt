package dev.phonelink.app.ui

import dev.phonelink.core.Device
import dev.phonelink.core.FileTransfer

/** A note typed on the computer and sent to this phone. */
data class Note(val from: String, val text: String)

/** What Android lets EchoTwin do; each missing item is a step in the setup checklist. */
data class Access(
    val notifications: Boolean,
    val battery: Boolean,
    val sms: Boolean,
    val bluetooth: Boolean,
    val phoneStatus: Boolean,
) {
    val missing: Int get() = listOf(notifications, battery, sms, bluetooth, phoneStatus).count { !it }
}

data class UiState(
    val myId: String,
    val devices: List<Device>,
    val transfers: List<FileTransfer>,
    val notes: List<Note>,
    val access: Access,
    val clipboardOn: Boolean,
    val queueOn: Boolean,
    val batteryAlertsOn: Boolean,
    /** This phone's battery (0–100, null if unknown), charging, and network label. */
    val myBattery: Int? = null,
    val myCharging: Boolean = false,
    val myNetwork: String = "",
    /** What plays on the computer (Remote screen), and its own buttons. */
    val computerMedia: dev.phonelink.core.PhoneMedia? = null,
    val commands: List<dev.phonelink.core.RemoteButton> = emptyList(),
    /** The computer's album art for `computerMedia` (null: none, or not for this track). */
    val computerArt: android.graphics.Bitmap? = null,
    /** When something last played on the computer (ms; 0 = never). */
    val computerPlayedMs: Long = 0L,
    /** Presentation app for Start: office | slides | canva | fullscreen. */
    val presentApp: String = "office",
    /** When a lock or unlock request last failed (ms; 0 = never), so the tile stops waiting. */
    val lockFailedMs: Long = 0L,
    /** Where received files are saved ("Download/EchoTwin" or the chosen folder), and if chosen. */
    val receiveFolder: String = "Download/EchoTwin",
    val receiveFolderChosen: Boolean = false,
    /** The pointer's look: dot | ring | spotlight, 0xRRGGBB, size 0..2. */
    val pointerShape: String = "dot",
    val pointerColor: Int = 0xFF3B30,
    val pointerSize: Int = 1,
    /** The connection switch (off: disconnected, nothing searching). */
    val connectionOn: Boolean,
    val busy: Boolean,
    /** Why the Bluetooth backup can't connect, if it can't. */
    val bluetoothProblem: String?,
    /** The Touchpad tab. */
    val touchpad: TouchpadStatus = TouchpadStatus(),
) {
    val touchpadPaired: Boolean get() = touchpad.paired
}

/**
 * The Touchpad tab: its own pairing with the laptop's touchpad service (not the main link).
 * `laptops` are names; `connectedTo` the one the pad drives now.
 */
data class TouchpadStatus(
    val started: Boolean = false,
    val startError: String? = null,
    val laptops: List<String> = emptyList(),
    val connectedTo: String? = null,
    /** The laptop made its virtual touchpad: fingers move the pointer now. */
    val padReady: Boolean = false,
    /** Why the laptop couldn't make it (shown in red). */
    val padError: String? = null,
    /** "Pairing…" while a scanned code is being used. */
    val pairing: String? = null,
) {
    val paired: Boolean get() = laptops.isNotEmpty()
}

/** Everything the screens can ask for; implemented by MainActivity. */
interface Actions {
    fun pair()
    fun reconnect()
    fun lock()
    /** Unlock the computer after the phone's fingerprint or screen lock; `onSent` once asked. */
    fun unlock(onSent: () -> Unit)
    /** The computer is locked but doesn't let this phone unlock it: say how to allow it. */
    fun explainUnlock()
    fun ring(on: Boolean)
    fun openButtons()
    fun setPresentApp(app: String)
    /** Pick the folder for received files (Android's folder chooser), or go back to the default. */
    fun chooseReceiveFolder()
    fun defaultReceiveFolder()
    /** Pointer at x, y (0..10000) or, `visible` false, gone. */
    fun pointer(x: Int, y: Int, visible: Boolean)
    fun setPointerStyle(shape: String, color: Int, size: Int)
    fun openPresenter()
    fun computerMedia(command: dev.phonelink.core.MediaCommand)
    fun runCommand(id: String)
    fun presentKey(key: dev.phonelink.core.PresentKeyFfi)
    fun sendClipboard()
    fun sendFiles()
    fun sendNote(text: String)
    fun copyNote(text: String)
    fun cancelTransfer(id: String)
    fun clearTransfers()
    fun setClipboard(on: Boolean)
    fun setQueue(on: Boolean)
    fun setBatteryAlerts(on: Boolean)
    fun setConnection(on: Boolean)
    fun allowNotifications()
    fun allowBattery()
    fun allowSms()
    fun allowBluetooth()
    fun allowPhoneStatus()
    fun testBluetooth()
    fun openAppInfo()
    fun unpair(device: Device)
    /** Touchpad tab: scan `phonelink touchpad pair`, forget its laptops. */
    fun scanTouchpad()
    fun forgetTouchpad()
    /** The pad is on screen at this size (mm) / left the screen. */
    fun openPad(widthMm: Float, heightMm: Float)
    fun closePad()
    /** Every touch event: the fingers down now. Never blocks. */
    fun touch(fingers: List<dev.phonelink.core.Finger>)
    /** Left click (`left`) or Right click went down or up. Never blocks. */
    fun button(left: Boolean, down: Boolean)
    /** Round trip to the laptop the pad drives, ms (null: not connected). Call off the main thread. */
    fun touchpadLatency(): Int?
}

val FileTransfer.isActive: Boolean get() = state == "active" || state == "waiting" || state == "queued"

fun FileTransfer.percent(): Int = if (size > 0uL) (done * 100uL / size).toInt() else 100

fun FileTransfer.statusText(): String = when (state) {
    "done" -> if (outgoing) "Sent to $deviceName" else "Saved in Download/EchoTwin"
    "active" -> "${percent()}%" + (etaSecs?.takeIf { rate > 0uL }?.let { " · " + formatEta(it.toLong()) } ?: "")
    "queued" -> "Queued"
    "waiting" -> (if (percent() > 0) "${percent()}% · " else "") + message.ifBlank { "Waiting" }
    "failed" -> "Failed: $message"
    else -> message.ifBlank { "Cancelled" }
}

/** "about 45 s left", "about 3 min left", "about 1 h 5 min left". */
fun formatEta(secs: Long): String = when {
    secs < 60 -> "about ${secs.coerceAtLeast(1)} s left"
    secs < 3600 -> "about ${(secs + 30) / 60} min left"
    else -> "about ${secs / 3600} h ${secs % 3600 / 60} min left"
}

/** What the Lock / Unlock tile is waiting for after a press. */
enum class LockPending { LOCKING, UNLOCKING }

/** What pressing the Lock / Unlock tile does. */
enum class LockDo { LOCK, UNLOCK, EXPLAIN, NOTHING }

/** The one Lock / Unlock tile: its label, whether it shows the open padlock, enabled, action. */
data class LockTile(val label: String, val open: Boolean, val enabled: Boolean, val does: LockDo)

/**
 * The tile from the computer's state, kept pure so the table is easy to check:
 * - offline or no lock support: "Lock", disabled
 * - waiting after a press: "Locking…" / "Unlocking…", disabled
 * - the computer hasn't said (older version, or its desktop doesn't tell): "Lock"
 * - unlocked: "Lock"
 * - locked and this phone may unlock it: "Unlock"
 * - locked, unlocking not allowed there: "Locked" (a tap explains how to allow it)
 */
fun lockTile(d: Device, pending: LockPending?): LockTile = when {
    !d.connected || !d.canLock -> LockTile("Lock", false, false, LockDo.NOTHING)
    pending == LockPending.LOCKING -> LockTile("Locking…", false, false, LockDo.NOTHING)
    pending == LockPending.UNLOCKING -> LockTile("Unlocking…", true, false, LockDo.NOTHING)
    d.locked != true -> LockTile("Lock", false, true, LockDo.LOCK)
    d.canUnlock && d.unlockAllowed -> LockTile("Unlock", true, true, LockDo.UNLOCK)
    else -> LockTile("Locked", true, true, LockDo.EXPLAIN)
}
