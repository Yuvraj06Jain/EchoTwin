package dev.phonelink.app.ui

import dev.phonelink.core.Device
import dev.phonelink.core.FileTransfer

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
    val notes: List<String>,
    val access: Access,
    val clipboardOn: Boolean,
    val queueOn: Boolean,
    /** The connection switch (off: disconnected, nothing searching). */
    val connectionOn: Boolean,
    val busy: Boolean,
    /** Why the Bluetooth backup can't connect, if it can't. */
    val bluetoothProblem: String?,
)

/** Everything the screens can ask for; implemented by MainActivity. */
interface Actions {
    fun pair()
    fun reconnect()
    fun lock()
    fun sendClipboard()
    fun sendFiles()
    fun sendNote(text: String)
    fun cancelTransfer(id: String)
    fun clearTransfers()
    fun setClipboard(on: Boolean)
    fun setQueue(on: Boolean)
    fun setConnection(on: Boolean)
    fun allowNotifications()
    fun allowBattery()
    fun allowSms()
    fun allowBluetooth()
    fun allowPhoneStatus()
    fun openAppInfo()
    fun unpair(device: Device)
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
