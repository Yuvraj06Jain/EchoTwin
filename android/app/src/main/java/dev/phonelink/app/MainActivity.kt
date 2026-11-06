package dev.phonelink.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.phonelink.core.Device
import dev.phonelink.core.PairingCode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
    private var pendingPair by mutableStateOf<Pair<String, PairingCode>?>(null)
    private var busy by mutableStateOf(false)
    private var notice by mutableStateOf<String?>(null)
    private var batteryOk by mutableStateOf(true)
    private var mirrorOn by mutableStateOf(true)
    private var clipboardOn by mutableStateOf(true)

    private val scan = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { onScanned(it) }
    }
    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    private var smsOn by mutableStateOf(true)

    private val askSms = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        smsOn = SmsSync.canRead()
        if (smsOn) {
            SmsSync.start()
            LinkManager.sendSmsSnapshot()
            notice = "Your SMS conversations now show on your computer"
        } else {
            notice = "SMS access not allowed. If Android says it's restricted, allow restricted settings in App info first."
        }
    }

    private var bluetoothOn by mutableStateOf(true)
    private val askBluetooth = registerForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        bluetoothOn = ok
        notice = if (ok) "Bluetooth backup ready" else "Without \"Nearby devices\", PhoneLink can't use the Bluetooth backup"
    }

    /** The system file picker; the permission to read the files is kept so sends can resume. */
    private val pickFiles = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isEmpty()) return@registerForActivityResult
        for (u in uris) {
            runCatching { contentResolver.takePersistableUriPermission(u, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        }
        LinkManager.sendFiles(uris, fromShare = false)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        ContextCompat.startForegroundService(this, Intent(this, LinkService::class.java))

        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                val devices by LinkManager.devices.collectAsStateWithLifecycle()
                val messages by LinkManager.messages.collectAsStateWithLifecycle()
                val myId by LinkManager.myId.collectAsStateWithLifecycle()
                val transfers by LinkManager.transfers.collectAsStateWithLifecycle()
                val snackbar = remember { SnackbarHostState() }
                LaunchedEffect(notice) {
                    notice?.let { snackbar.showSnackbar(it); notice = null }
                }
                Scaffold(snackbarHost = { SnackbarHost(snackbar) }) { padding ->
                    HomeScreen(
                        modifier = Modifier.padding(padding),
                        myId = myId,
                        devices = devices,
                        messages = messages,
                        busy = busy,
                        batteryOk = batteryOk,
                        mirrorOn = mirrorOn,
                        onAllowMirror = ::openNotificationAccess,
                        onOpenAppInfo = ::openAppInfo,
                        onPair = ::startScan,
                        onSend = ::sendText,
                        onUnpair = ::unpair,
                        onReconnect = { LinkManager.nudge(); notice = "Looking for your computer on this network…" },
                        onFixBattery = ::fixBattery,
                        clipboardOn = clipboardOn,
                        onClipboardSharing = ::setClipboardSharing,
                        onLock = { LinkManager.lockComputer() },
                        onSendClipboard = ::sendClipboard,
                        transfers = transfers,
                        onSendFiles = { pickFiles.launch(arrayOf("*/*")) },
                        onCancelTransfer = { LinkManager.cancelTransfer(it) },
                        smsOn = smsOn,
                        onAllowSms = { askSms.launch(SmsSync.permissions) },
                        bluetoothOn = bluetoothOn,
                        onAllowBluetooth = { askBluetooth.launch(BluetoothBackup.permission) },
                    )
                    pendingPair?.let { (uri, code) ->
                        ConfirmPairDialog(
                            code = code,
                            onConfirm = { pendingPair = null; pair(uri) },
                            onDismiss = { pendingPair = null },
                        )
                    }
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        batteryOk = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
        mirrorOn = LinkManager.mirrorEnabled(this)
        clipboardOn = Prefs.clipboardSharing(this)
        smsOn = SmsSync.canRead()
        bluetoothOn = BluetoothBackup.allowed()
        lifecycleScope.launch(Dispatchers.IO) { runCatching { LinkManager.link() }; LinkManager.refresh() }
        LinkManager.nudge() // opening the app = "connect now", no waiting for the next retry
    }

    private fun startScan() {
        scan.launch(
            ScanOptions()
                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt("Scan the code shown by `phonelink pair` on your computer")
                .setBeepEnabled(false)
                .setOrientationLocked(true)
        )
    }

    private fun onScanned(contents: String) {
        giveUpCamera()
        lifecycleScope.launch {
            val code = withContext(Dispatchers.IO) { runCatching { LinkManager.link().readPairingCode(contents) } }
            code.onSuccess { pendingPair = contents to it }
                .onFailure { notice = "That is not a PhoneLink pairing code." }
        }
    }

    private fun pair(uri: String) {
        lifecycleScope.launch {
            busy = true
            val result = withContext(Dispatchers.IO) { runCatching { LinkManager.link().pair(uri) } }
            busy = false
            notice = result.fold(
                onSuccess = { "Paired with ${it.name}" },
                onFailure = { "Pairing failed: ${it.message}" },
            )
            LinkManager.refresh()
        }
    }

    private fun sendText(text: String) {
        lifecycleScope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { LinkManager.link().sendText(null, text) } }
            notice = result.fold(
                onSuccess = { now -> if (now) "Sent" else "Computer offline; it will get this when it reconnects" },
                onFailure = { "Could not send: ${it.message}" },
            )
            LinkManager.refresh()
        }
    }

    private fun setClipboardSharing(on: Boolean) {
        Prefs.setClipboardSharing(this, on)
        clipboardOn = on
        LinkManager.settingsChanged()
    }

    /** This screen is in front, so it may read the clipboard directly. */
    private fun sendClipboard() {
        val clip = getSystemService(ClipboardManager::class.java)?.primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()
        if (text.isNullOrBlank()) {
            notice = "The clipboard is empty"
        } else {
            LinkManager.sendClipboard(text)
        }
    }

    private fun unpair(device: Device) {
        lifecycleScope.launch {
            withContext(Dispatchers.IO) { runCatching { LinkManager.link().unpair(device.id) } }
            LinkManager.refresh()
            notice = "Unpaired ${device.name}"
        }
    }

    /**
     * The camera is only needed for the pairing scan. From Android 13 we hand the permission
     * back: Android removes it as soon as this app's process ends, and the next pairing
     * asks again. The app never uses the camera (or microphone) in the background.
     */
    private fun giveUpCamera() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        ) {
            revokeSelfPermissionOnKill(Manifest.permission.CAMERA)
        }
    }

    private fun openNotificationAccess() {
        val general = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
        if (Build.VERSION.SDK_INT < 30) {
            startActivity(general)
            return
        }
        // Android 11+: straight to PhoneLink's own switch.
        val component = ComponentName(this, MirrorService::class.java).flattenToString()
        val detail = Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
            .putExtra(Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME, component)
        try {
            startActivity(detail)
        } catch (e: Exception) {
            startActivity(general)
        }
    }

    private fun openAppInfo() {
        startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
    }

    @SuppressLint("BatteryLife") // sideloaded personal app: this is the intended use
    private fun fixBattery() {
        startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
        )
    }
}

@Composable
private fun HomeScreen(
    modifier: Modifier,
    myId: String,
    devices: List<Device>,
    messages: List<String>,
    busy: Boolean,
    batteryOk: Boolean,
    mirrorOn: Boolean,
    onAllowMirror: () -> Unit,
    onOpenAppInfo: () -> Unit,
    onPair: () -> Unit,
    onSend: (String) -> Unit,
    onUnpair: (Device) -> Unit,
    onReconnect: () -> Unit,
    onFixBattery: () -> Unit,
    clipboardOn: Boolean,
    onClipboardSharing: (Boolean) -> Unit,
    onLock: () -> Unit,
    onSendClipboard: () -> Unit,
    transfers: List<dev.phonelink.core.FileTransfer>,
    onSendFiles: () -> Unit,
    onCancelTransfer: (String) -> Unit,
    smsOn: Boolean,
    onAllowSms: () -> Unit,
    bluetoothOn: Boolean,
    onAllowBluetooth: () -> Unit,
) {
    var draft by remember { mutableStateOf("") }
    var confirmUnpair by remember { mutableStateOf<Device?>(null) }

    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Spacer(Modifier.padding(top = 8.dp))
            Text("PhoneLink", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold)
            Text("This phone's id: $myId", style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
        }

        if (devices.isNotEmpty() && !mirrorOn) {
            item {
                Card {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Show this phone's notifications on your computer", fontWeight = FontWeight.Bold)
                        Text(
                            "Allow PhoneLink to read notifications. They go only to your paired computer, " +
                                "encrypted, and nothing from the computer is shown here."
                        )
                        Button(onClick = onAllowMirror) { Text("Allow notification access") }
                        Text(
                            "Switch greyed out? Android blocks this for apps installed from a file. Open App info, " +
                                "tap ⋮ (top right) → Allow restricted settings, then try again.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        OutlinedButton(onClick = onOpenAppInfo) { Text("Open App info") }
                    }
                }
            }
        }

        if (devices.isNotEmpty() && !bluetoothOn) {
            item {
                Card {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Stay connected over Bluetooth", fontWeight = FontWeight.Bold)
                        Text(
                            "When Wi-Fi is gone, PhoneLink can reach your computer over Bluetooth (pair them in " +
                                "Bluetooth settings first). It only connects to your paired computer: no scanning, " +
                                "no location."
                        )
                        Button(onClick = onAllowBluetooth) { Text("Allow Nearby devices") }
                    }
                }
            }
        }

        if (devices.isNotEmpty() && !smsOn) {
            item {
                Card {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Read and send SMS from your computer", fontWeight = FontWeight.Bold)
                        Text(
                            "Allow PhoneLink to read SMS, send SMS and see contact names. Conversations go only to " +
                                "your paired computer, encrypted, and are not stored there. RCS chats can't be shown."
                        )
                        Button(onClick = onAllowSms) { Text("Allow SMS access") }
                    }
                }
            }
        }

        if (!batteryOk) {
            item {
                Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer)) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Keep PhoneLink connected", fontWeight = FontWeight.Bold)
                        Text(
                            "Samsung phones stop background apps. Allow PhoneLink to run unrestricted, then also add " +
                                "it under Settings → Battery → Background usage limits → Never sleeping apps."
                        )
                        Button(onClick = onFixBattery) { Text("Allow background use") }
                    }
                }
            }
        }

        item { Text("Computers", style = MaterialTheme.typography.titleMedium) }
        if (devices.isEmpty()) {
            item { Text("No computer paired yet. On your computer run `phonelink pair`, then tap the button below.") }
        }
        items(devices, key = { it.id }) { d ->
            Card(Modifier.fillMaxWidth()) {
                Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(d.name, fontWeight = FontWeight.Bold)
                        Text(d.id, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
                        val state = when {
                            d.connected && d.via == "bluetooth" -> "Connected over Bluetooth"
                            d.connected -> "Connected"
                            else -> BluetoothBackup.problem?.let { "Offline · $it" } ?: "Offline"
                        }
                        val queued = if (d.pending > 0uL) " · ${d.pending} queued" else ""
                        Text(state + queued, color = if (d.connected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                    }
                    if (!d.connected) TextButton(onClick = onReconnect) { Text("Reconnect") }
                    TextButton(onClick = { confirmUnpair = d }) { Text("Unpair") }
                }
            }
        }
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                OutlinedButton(onClick = onPair, enabled = !busy) { Text("Pair a computer") }
                if (busy) {
                    Spacer(Modifier.width(12.dp))
                    CircularProgressIndicator()
                }
            }
        }

        if (devices.isNotEmpty()) {
            val canLock = devices.any { it.canLock }
            val canClip = devices.any { it.canClipboard }
            item {
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Your computer", fontWeight = FontWeight.Bold)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = onLock, enabled = canLock) { Text("Lock computer") }
                            OutlinedButton(onClick = onSendClipboard, enabled = canClip && clipboardOn) {
                                Text("Send clipboard")
                            }
                        }
                        OutlinedButton(onClick = onSendFiles) { Text("Send files") }
                        Text(
                            "Or use Share → Send to computer in any app. Files from the computer are saved in Download/PhoneLink.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text("Share clipboard")
                                Text(
                                    "Text you copy on the computer can be pasted here. To send text from " +
                                        "here, tap Send clipboard (also on the PhoneLink notification and as a " +
                                        "Quick Settings tile).",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            Switch(checked = clipboardOn, onCheckedChange = onClipboardSharing)
                        }
                        if (!canLock && devices.any { it.connected }) {
                            Text(
                                "Update PhoneLink on the computer to lock it and share the clipboard from here.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Test message", style = MaterialTheme.typography.titleMedium)
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = { Text("Hello from my phone") },
                        singleLine = true,
                    )
                    Button(onClick = { onSend(draft.ifBlank { "Hello from my phone" }); draft = "" }) { Text("Send to computer") }
                }
            }
        }

        if (transfers.isNotEmpty()) {
            item { Text("Files", style = MaterialTheme.typography.titleMedium) }
            items(transfers.take(10), key = { "t-" + it.id }) { t ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text((if (t.outgoing) "↑ " else "↓ ") + t.name, fontWeight = FontWeight.Bold, maxLines = 1)
                        val pct = if (t.size > 0uL) (t.done * 100uL / t.size).toInt() else 100
                        val status = when (t.state) {
                            "done" -> if (t.outgoing) "Sent" else "Saved in Download/PhoneLink"
                            "active" -> "$pct%"
                            "queued" -> "Queued"
                            "waiting" -> (if (pct > 0) "$pct% · " else "") + t.message.ifBlank { "Waiting" }
                            "failed" -> "Failed: " + t.message
                            else -> t.message.ifBlank { "Cancelled" }
                        }
                        Text(status, style = MaterialTheme.typography.bodySmall, maxLines = 2)
                    }
                    if (t.state == "active" || t.state == "waiting" || t.state == "queued") {
                        TextButton(onClick = { onCancelTransfer(t.id) }) { Text("Cancel") }
                    }
                }
            }
        }

        if (messages.isNotEmpty()) {
            item { Text("Received", style = MaterialTheme.typography.titleMedium) }
            items(messages) { m -> Text(m) }
        }
        item { Spacer(Modifier.padding(bottom = 24.dp)) }
    }

    confirmUnpair?.let { d ->
        AlertDialog(
            onDismissRequest = { confirmUnpair = null },
            title = { Text("Unpair ${d.name}?") },
            text = { Text("It will no longer be able to connect. You can pair again later with a new QR code.") },
            confirmButton = { TextButton(onClick = { confirmUnpair = null; onUnpair(d) }) { Text("Unpair") } },
            dismissButton = { TextButton(onClick = { confirmUnpair = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun ConfirmPairDialog(code: PairingCode, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Pair with ${code.name}?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Check that your computer's terminal shows exactly this id:")
                Text(code.id, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold)
            }
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Pair") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
