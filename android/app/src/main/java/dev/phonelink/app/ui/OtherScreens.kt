package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phonelink.core.Device
import dev.phonelink.core.PairingCode

// ------------------------------------------------------------------ Files

@Composable
fun FilesScreen(state: UiState, actions: Actions, modifier: Modifier = Modifier) {
    val bluetoothOnly = state.devices.isNotEmpty() &&
        state.devices.none { it.connected && it.via != "bluetooth" } && state.devices.any { it.connected }
    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item { Header("Files") }
        item {
            Panel {
                Button(
                    onClick = actions::sendFiles,
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(10.dp),
                ) { Text("Send files to computer") }
                Hint(
                    if (bluetoothOnly) "On Bluetooth, files wait for Wi-Fi and then send by themselves."
                    else "Or use Share → EchoTwin in any app. Files from the computer are saved in Download/EchoTwin."
                )
            }
        }
        if (state.transfers.isEmpty()) {
            item { Hint("Nothing sent or received yet.") }
        } else {
            item {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.weight(1f)) { SectionTitle("Recent") }
                    if (state.transfers.any { !it.isActive }) {
                        TextButton(onClick = actions::clearTransfers) { Text("Clear finished", color = Pl.Muted) }
                    }
                }
            }
            items(state.transfers, key = { it.id }) { TransferCard(it, actions) }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }
}

// ------------------------------------------------------------------ Settings

@Composable
fun SettingsScreen(state: UiState, actions: Actions, onOpenSetup: () -> Unit, modifier: Modifier = Modifier) {
    var confirmUnpair by remember { mutableStateOf<Device?>(null) }
    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item { Header("Settings") }
        item { SectionTitle("Sharing") }
        item {
            Panel {
                SwitchRow(
                    "Share clipboard",
                    "Text copied on the computer can be pasted here. Send text from here with the " +
                        "Clipboard button, the EchoTwin notification or the Quick Settings tile.",
                    checked = state.clipboardOn,
                ) { actions.setClipboard(it) }
                SwitchRow(
                    "Connection",
                    "Off: disconnect from the computer and stop running in the background " +
                        "(saves battery). The computer stops searching too.",
                    checked = state.connectionOn,
                ) { actions.setConnection(it) }
                SwitchRow(
                    "Queue notifications while disconnected",
                    "Off: notifications that arrive while the computer is away are not sent later " +
                        "(you already saw them here). On: they're delivered when it reconnects.",
                    checked = state.queueOn,
                ) { actions.setQueue(it) }
            }
        }
        item { SectionTitle("Bluetooth backup") }
        item {
            Panel {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("When Wi-Fi is gone", fontWeight = FontWeight.Medium)
                        Hint(
                            when {
                                dev.phonelink.app.BluetoothBackup.active -> "Connected over Bluetooth now"
                                !state.access.bluetooth -> "Needs \"Nearby devices\" (Finish setup)"
                                else -> state.bluetoothProblem ?: "Ready: takes over 15 s after Wi-Fi drops"
                            }
                        )
                    }
                    TextButton(onClick = actions::testBluetooth) { Text("Test", color = Pl.AccentText) }
                }
            }
        }
        item { SectionTitle("Access") }
        item {
            Panel(Modifier.clickable(onClick = onOpenSetup)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Permissions and background use", fontWeight = FontWeight.Medium)
                        Hint(
                            if (state.access.missing == 0) "Everything allowed"
                            else "${state.access.missing} not allowed yet"
                        )
                    }
                    Text("Open", color = Pl.AccentText)
                }
            }
        }
        item { SectionTitle("Computers") }
        items(state.devices, key = { it.id }) { d ->
            Panel {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(d.name, fontWeight = FontWeight.Medium)
                        Text(d.id, color = Pl.Muted, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                        Hint(statusLabel(d))
                    }
                    TextButton(onClick = { confirmUnpair = d }) { Text("Unpair", color = Pl.Bad) }
                }
            }
        }
        item {
            OutlinedButton(
                onClick = actions::pair,
                enabled = !state.busy,
                border = BorderStroke(1.dp, Pl.Line),
                shape = RoundedCornerShape(10.dp),
            ) { Text("Pair another computer", color = Pl.Text) }
        }
        item { SectionTitle("This phone") }
        item {
            Panel {
                Hint("Id (check it matches on the computer)")
                Text(state.myId, fontFamily = FontFamily.Monospace)
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }

    confirmUnpair?.let { d ->
        AlertDialog(
            onDismissRequest = { confirmUnpair = null },
            containerColor = Pl.Panel,
            title = { Text("Unpair ${d.name}?") },
            text = { Text("It will no longer be able to connect. You can pair again later with a new QR code.") },
            confirmButton = {
                TextButton(onClick = { confirmUnpair = null; actions.unpair(d) }) { Text("Unpair", color = Pl.Bad) }
            },
            dismissButton = { TextButton(onClick = { confirmUnpair = null }) { Text("Cancel", color = Pl.Muted) } },
        )
    }
}

// ------------------------------------------------------------------ Setup checklist

@Composable
fun SetupScreen(access: Access, actions: Actions, onDone: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Header("Finish setup")
        Hint("A few Android permissions so EchoTwin stays connected and can show everything on your computer.")
        Panel {
            Step(
                "Notification access",
                "Show this phone's notifications on the computer.",
                access.notifications, actions::allowNotifications,
            )
            Step(
                "Run in the background",
                "Samsung stops background apps. Also add EchoTwin under Settings → Battery → " +
                    "Background usage limits → Never sleeping apps.",
                access.battery, actions::allowBattery,
            )
            Step("SMS and contacts", "Read and answer texts from the computer.", access.sms, actions::allowSms)
            Step(
                "Nearby devices",
                "Bluetooth backup when Wi-Fi is gone. Only your paired computer: no scanning, no location.",
                access.bluetooth, actions::allowBluetooth,
            )
            Step(
                "Phone status",
                "Show 4G/5G and signal on the computer next to the battery. Only the network type " +
                    "and signal are read; nothing is sent anywhere else.",
                access.phoneStatus, actions::allowPhoneStatus,
            )
        }
        Panel(color = Pl.WarnSoft, border = Pl.Warn.copy(alpha = 0.4f)) {
            Text("Switch greyed out?", fontWeight = FontWeight.Medium)
            Hint(
                "Android restricts apps installed from a file. Open App info, tap ⋮ (top right) → " +
                    "Allow restricted settings, then come back and tap Allow again.",
                color = Pl.Text,
            )
            TextButton(onClick = actions::openAppInfo) { Text("Open App info", color = Pl.Warn) }
        }
        Button(onClick = onDone, modifier = Modifier.fillMaxWidth(), shape = RoundedCornerShape(10.dp)) {
            Text(if (access.missing == 0) "Done" else "Later")
        }
        Spacer(Modifier.height(16.dp))
    }
}

@Composable
private fun Step(title: String, text: String, done: Boolean, onAllow: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Surface(
            shape = RoundedCornerShape(50),
            color = if (done) Pl.Accent else Pl.Raised,
            modifier = Modifier.size(24.dp),
        ) {
            if (done) Icon(Icons.Filled.Check, null, tint = Pl.Bg, modifier = Modifier.padding(4.dp))
        }
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(title, fontWeight = FontWeight.Medium, color = if (done) Pl.Muted else Pl.Text)
            Hint(text)
        }
        if (!done) {
            Button(
                onClick = onAllow,
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.buttonColors(containerColor = Pl.AccentSoft, contentColor = Pl.AccentText),
            ) { Text("Allow") }
        }
    }
}

// ------------------------------------------------------------------ Pairing

@Composable
fun PairScreen(busy: Boolean, onScan: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Spacer(Modifier.height(24.dp))
        Text("EchoTwin", fontSize = 28.sp, fontWeight = FontWeight.Bold)
        Hint("Pair with your computer", color = Pl.Text)
        Surface(
            modifier = Modifier.size(220.dp).clickable(enabled = !busy, onClick = onScan),
            shape = RoundedCornerShape(20.dp),
            color = Pl.Panel,
            border = BorderStroke(2.dp, Pl.Accent),
        ) {
            Box(contentAlignment = Alignment.Center) {
                if (busy) CircularProgressIndicator(color = Pl.AccentText)
                else Text("Tap to scan\nthe QR code", color = Pl.AccentText, textAlign = TextAlign.Center)
            }
        }
        Panel {
            Hint("1. On your computer, open EchoTwin and choose Pair (or run `phonelink pair`).", color = Pl.Text)
            Hint("2. Scan the code it shows.", color = Pl.Text)
            Hint("3. Check that both screens show the same id.", color = Pl.Text)
        }
        Button(
            onClick = onScan,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(10.dp),
        ) { Text("Scan code") }
        Hint(
            "The camera is used only for this scan and Android takes the permission back afterwards.",
        )
    }
}

@Composable
fun ConfirmPairDialog(code: PairingCode, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Pl.Panel,
        title = { Text("Pair with ${code.name}?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Check that your computer shows exactly this id:")
                Surface(color = Pl.Raised, shape = RoundedCornerShape(10.dp)) {
                    Text(
                        code.id,
                        fontFamily = FontFamily.Monospace,
                        fontWeight = FontWeight.Bold,
                        fontSize = 17.sp,
                        color = Pl.AccentText,
                        modifier = Modifier.padding(12.dp).fillMaxWidth(),
                        textAlign = TextAlign.Center,
                    )
                }
            }
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Pair", color = Pl.AccentText) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = Pl.Muted) } },
    )
}
