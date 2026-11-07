package dev.phonelink.app.ui

import dev.phonelink.core.Device
import dev.phonelink.core.FileTransfer

/** What Android lets PhoneLink do; each missing item is a step in the setup checklist. */
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
    "done" -> if (outgoing) "Sent to $deviceName" else "Saved in Download/PhoneLink"
    "active" -> "${percent()}%"
    "queued" -> "Queued"
    "waiting" -> (if (percent() > 0) "${percent()}% · " else "") + message.ifBlank { "Waiting" }
    "failed" -> "Failed: $message"
    else -> message.ifBlank { "Cancelled" }
}
