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
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.List
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.phonelink.app.ui.Access
import dev.phonelink.app.ui.Actions
import dev.phonelink.app.ui.ConfirmPairDialog
import dev.phonelink.app.ui.FilesScreen
import dev.phonelink.app.ui.HomeScreen
import dev.phonelink.app.ui.PairScreen
import dev.phonelink.app.ui.PhoneLinkTheme
import dev.phonelink.app.ui.Pl
import dev.phonelink.app.ui.SettingsScreen
import dev.phonelink.app.ui.SetupScreen
import dev.phonelink.app.ui.UiState
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
    private var queueOn by mutableStateOf(false)
    private var pairedOnDisk by mutableStateOf(false)

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

    private var phoneStatusOn by mutableStateOf(true)
    private val askPhoneStatus = registerForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        phoneStatusOn = ok
        if (ok) StatusWatcher.start(this)
        notice = if (ok) "Your computer will show 4G/5G" else "Without it, the computer shows only Wi-Fi or Mobile data"
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
            PhoneLinkTheme {
                val devices by LinkManager.devices.collectAsStateWithLifecycle()
                val notes by LinkManager.messages.collectAsStateWithLifecycle()
                val myId by LinkManager.myId.collectAsStateWithLifecycle()
                val transfers by LinkManager.transfers.collectAsStateWithLifecycle()
                val state = UiState(
                    myId = myId,
                    devices = devices,
                    transfers = transfers,
                    notes = notes,
                    access = Access(mirrorOn, batteryOk, smsOn, bluetoothOn, phoneStatusOn),
                    clipboardOn = clipboardOn,
                    queueOn = queueOn,
                    busy = busy,
                    bluetoothProblem = BluetoothBackup.problem,
                )
                val snackbar = remember { SnackbarHostState() }
                LaunchedEffect(notice) {
                    notice?.let { snackbar.showSnackbar(it); notice = null }
                }
                var tab by rememberSaveable { mutableIntStateOf(0) }
                var setupOpen by rememberSaveable { mutableStateOf(false) }
                BackHandler(enabled = setupOpen || tab != 0) { if (setupOpen) setupOpen = false else tab = 0 }
                // Until the core has started, trust the saved pairing file (no flash of the pair screen).
                val paired = if (myId.isNotEmpty()) devices.isNotEmpty() else pairedOnDisk
                Scaffold(
                    containerColor = Pl.Bg,
                    snackbarHost = { SnackbarHost(snackbar) },
                    bottomBar = {
                        if (paired && !setupOpen) BottomBar(tab) { tab = it }
                    },
                ) { padding ->
                    val m = Modifier.padding(padding)
                    when {
                        !paired -> PairScreen(busy, ::startScan, m)
                        setupOpen -> SetupScreen(state.access, actions, { setupOpen = false }, m)
                        tab == 1 -> FilesScreen(state, actions, m)
                        tab == 2 -> SettingsScreen(state, actions, { setupOpen = true }, m)
                        else -> HomeScreen(state, actions, { setupOpen = true }, m)
                    }
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

    private val actions = object : Actions {
        override fun pair() = startScan()
        override fun reconnect() {
            LinkManager.nudge()
            notice = "Looking for your computer on this network…"
        }
        override fun lock() = LinkManager.lockComputer()
        override fun sendClipboard() = this@MainActivity.sendClipboard()
        override fun sendFiles() = pickFiles.launch(arrayOf("*/*"))
        override fun sendNote(text: String) = sendText(text)
        override fun cancelTransfer(id: String) = LinkManager.cancelTransfer(id)
        override fun clearTransfers() = LinkManager.clearTransfers()
        override fun setClipboard(on: Boolean) = setClipboardSharing(on)
        override fun setQueue(on: Boolean) {
            LinkManager.setQueueNotifications(on)
            queueOn = on
        }
        override fun allowNotifications() = openNotificationAccess()
        override fun allowBattery() = fixBattery()
        override fun allowSms() = askSms.launch(SmsSync.permissions)
        override fun allowBluetooth() = askBluetooth.launch(BluetoothBackup.permission)
        override fun allowPhoneStatus() = askPhoneStatus.launch(StatusWatcher.PHONE_PERMISSION)
        override fun openAppInfo() = this@MainActivity.openAppInfo()
        override fun unpair(device: Device) = this@MainActivity.unpair(device)
    }

    override fun onResume() {
        super.onResume()
        batteryOk = getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
        mirrorOn = LinkManager.mirrorEnabled(this)
        clipboardOn = Prefs.clipboardSharing(this)
        queueOn = Prefs.queueNotifications(this)
        pairedOnDisk = LinkManager.hasPairedDevice(this)
        smsOn = SmsSync.canRead()
        bluetoothOn = BluetoothBackup.allowed()
        phoneStatusOn = StatusWatcher.allowed(this)
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
private fun BottomBar(selected: Int, onSelect: (Int) -> Unit) {
    val items = listOf(
        "Home" to Icons.Filled.Home,
        "Files" to Icons.AutoMirrored.Filled.List,
        "Settings" to Icons.Filled.Settings,
    )
    NavigationBar(containerColor = Pl.Panel) {
        items.forEachIndexed { i, (label, icon) ->
            NavigationBarItem(
                selected = selected == i,
                onClick = { onSelect(i) },
                icon = { Icon(icon, contentDescription = null) },
                label = { Text(label) },
                colors = NavigationBarItemDefaults.colors(
                    selectedIconColor = Pl.AccentText,
                    selectedTextColor = Pl.AccentText,
                    indicatorColor = Pl.AccentSoft,
                    unselectedIconColor = Pl.Muted,
                    unselectedTextColor = Pl.Muted,
                ),
            )
        }
    }
}
