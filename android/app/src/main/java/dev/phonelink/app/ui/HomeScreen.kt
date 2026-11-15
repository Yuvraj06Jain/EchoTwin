package dev.phonelink.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phonelink.core.Device
import dev.phonelink.core.FileTransfer

@Composable
fun HomeScreen(state: UiState, actions: Actions, onOpenSetup: () -> Unit, modifier: Modifier = Modifier) {
    var writingNote by remember { mutableStateOf(false) }
    val main = state.devices.firstOrNull { it.connected } ?: state.devices.firstOrNull()
    val others = state.devices.filter { it != main }
    val active = state.transfers.filter { it.isActive }

    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item { Header("EchoTwin") }

        if (state.access.missing > 0) {
            item {
                Panel(Modifier.clickable(onClick = onOpenSetup), color = Pl.AccentSoft, border = Pl.Accent) {
                    Text("Finish setup", fontWeight = FontWeight.SemiBold, color = Pl.AccentText)
                    val n = state.access.missing
                    Hint("$n ${if (n == 1) "step" else "steps"} left so everything works. Tap to continue.")
                }
            }
        }

        if (main != null) {
            val bluetooth = main.connected && main.via == "bluetooth"
            item {
                TwinHeader(
                    computer = main.name,
                    computerBattery = main.battery?.toInt(),
                    computerCharging = main.charging,
                    myBattery = state.myBattery,
                    myCharging = state.myCharging,
                    myNetwork = state.myNetwork,
                    link = when {
                        !state.connectionOn -> Link.Off
                        bluetooth -> Link.Bluetooth
                        main.connected -> Link.Wifi
                        main.pausedByPeer -> Link.PausedThere
                        else -> Link.Offline
                    },
                )
            }
            item { ConnectionTile(main, state, actions) }
            if (bluetooth) {
                item {
                    WarnCard(
                        "Using the Bluetooth backup. Notifications, messages and the clipboard still " +
                            "work; files wait for Wi-Fi."
                    )
                }
            }
            item { ActionGrid(main, state, actions, onNote = { writingNote = true }) }
        }

        if (active.isNotEmpty()) {
            item { SectionTitle("Now") }
            items(active.take(3), key = { "now-" + it.id }) { TransferCard(it, actions) }
        }

        item { SectionTitle("Shows on your computer") }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ShowChip("Notifications", state.access.notifications, Modifier.weight(1f)) { actions.allowNotifications() }
                ShowChip("Messages", state.access.sms, Modifier.weight(1f)) {
                    if (state.access.sms) actions.openAppInfo() else actions.allowSms()
                }
                ShowChip("Clipboard", state.clipboardOn, Modifier.weight(1f)) { actions.setClipboard(!state.clipboardOn) }
            }
        }

        if (others.isNotEmpty()) {
            item { SectionTitle("Other computers") }
            items(others, key = { "o-" + it.id }) { d ->
                Panel {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(d.name, fontWeight = FontWeight.SemiBold)
                            Hint(statusLabel(d))
                        }
                        if (!d.connected) TextButton(onClick = actions::reconnect) { Text("Reconnect") }
                    }
                }
            }
        }

        if (state.notes.isNotEmpty()) {
            item { SectionTitle("From your computer") }
            items(state.notes.take(5)) { Panel { Text(it) } }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }

    if (writingNote) NoteDialog(onSend = { writingNote = false; actions.sendNote(it) }, onDismiss = { writingNote = false })
}

@Composable
fun Header(title: String) {
    Text(
        title,
        fontSize = 26.sp,
        fontWeight = FontWeight.Bold,
        modifier = Modifier.padding(top = 18.dp, bottom = 4.dp, start = 4.dp),
    )
}

fun statusLabel(d: Device, connectionOn: Boolean = true): String = when {
    !connectionOn -> "Connection off"
    d.connected && d.via == "bluetooth" -> "Connected · Bluetooth"
    d.connected -> "Connected · Wi-Fi"
    d.pausedByPeer -> "Switched off on the computer"
    else -> "Offline"
}

/** The connection switch, with what's going on and Reconnect when it can't reach the computer. */
@Composable
private fun ConnectionTile(d: Device, state: UiState, actions: Actions) {
    val bluetooth = d.connected && d.via == "bluetooth"
    Panel {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconSquare(Icons.Filled.Refresh, if (d.connected) Pl.AccentText else Pl.Muted)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text("Connection", fontWeight = FontWeight.SemiBold)
                Hint(if (state.connectionOn) "On · reconnects by itself" else "Off · nothing runs")
            }
            SwitchOnly(state.connectionOn) { actions.setConnection(it) }
        }
        when {
            !state.connectionOn -> {}
            bluetooth -> Hint("Wi-Fi can't reach the computer right now.")
            d.pausedByPeer && !d.connected -> Hint("The connection was switched off on the computer. It reconnects as soon as it's turned back on there.")
            !d.connected -> Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) {
                    Hint(
                        (state.bluetoothProblem?.let { "$it. " } ?: "") +
                            "Make sure both are on the same Wi-Fi (or your hotspot)." +
                            if (d.pending > 0uL) " ${d.pending} waiting to send." else ""
                    )
                }
                TextButton(onClick = actions::reconnect) { Text("Reconnect", color = Pl.AccentText) }
            }
        }
    }
}

/** Lock, Clipboard, Send files / Note, Ring: three to a row. */
@Composable
private fun ActionGrid(d: Device, state: UiState, actions: Actions, onNote: () -> Unit) {
    val bluetooth = d.connected && d.via == "bluetooth"
    var ringing by remember { mutableStateOf(false) }
    val tiles = buildList<@Composable (Modifier) -> Unit> {
        add { m -> ActionTile("Lock", Icons.Filled.Lock, d.canLock, m, actions::lock) }
        add { m -> ActionTile("Clipboard", Icons.Filled.Share, d.canClipboard && state.clipboardOn, m, actions::sendClipboard) }
        add { m -> ActionTile("Send files", Icons.AutoMirrored.Filled.Send, !bluetooth, m, actions::sendFiles) }
        add { m -> ActionTile("Note", Icons.Filled.Edit, true, m, onNote) }
        if (d.canRemote) {
            add { m -> ActionTile("Remote", Icons.Filled.PlayArrow, true, m, actions::openRemote) }
        }
        if (d.canRing) {
            add { m ->
                ActionTile(if (ringing) "Stop" else "Ring", Icons.Filled.Notifications, true, m) {
                    ringing = !ringing
                    actions.ring(ringing)
                }
            }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        tiles.chunked(3).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                row.forEach { it(Modifier.weight(1f)) }
                repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
            }
        }
    }
}

/** A tinted rounded square holding an icon. */
@Composable
private fun IconSquare(icon: ImageVector, tint: Color) {
    Box(
        Modifier.size(36.dp).background(tint.copy(alpha = 0.16f), RoundedCornerShape(10.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(20.dp))
    }
}

@Composable
private fun SwitchOnly(checked: Boolean, onChange: (Boolean) -> Unit) {
    Switch(
        checked = checked,
        onCheckedChange = onChange,
        colors = SwitchDefaults.colors(
            checkedTrackColor = Pl.Accent,
            checkedThumbColor = Color.White,
            uncheckedTrackColor = Pl.Raised,
            uncheckedBorderColor = Pl.Line,
            uncheckedThumbColor = Pl.Muted,
        ),
    )
}

/** "Notifications ●": what this phone shows on the computer; a tap changes it. */
@Composable
private fun ShowChip(label: String, on: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Row(
        modifier
            .background(Pl.Panel, RoundedCornerShape(14.dp))
            .border(1.dp, Pl.Edge, RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(8.dp).background(if (on) Pl.AccentText else Pl.Muted.copy(alpha = 0.5f), CircleShape))
        Spacer(Modifier.width(7.dp))
        Text(label, fontSize = 12.sp, color = if (on) Pl.Text else Pl.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun ActionTile(label: String, icon: ImageVector, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val tint = if (enabled) Pl.AccentText else Pl.Muted.copy(alpha = 0.5f)
    Surface(
        modifier = modifier.clickable(enabled = enabled, onClick = onClick),
        color = Pl.Panel,
        shape = RoundedCornerShape(18.dp),
        border = BorderStroke(1.dp, Pl.Edge),
    ) {
        Column(
            Modifier.padding(vertical = 14.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            IconSquare(icon, tint)
            Text(label, fontSize = 12.sp, color = if (enabled) Pl.Text else Pl.Muted, maxLines = 1)
        }
    }
}

@Composable
fun TransferCard(t: FileTransfer, actions: Actions) {
    Panel {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(
                    (if (t.outgoing) "↑ " else "↓ ") + t.name,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Hint(t.statusText(), color = if (t.state == "failed") Pl.Bad else Pl.Muted)
            }
            if (t.isActive) TextButton(onClick = { actions.cancelTransfer(t.id) }) { Text("Cancel", color = Pl.Muted) }
        }
        if (t.isActive) {
            LinearProgressIndicator(
                progress = { t.percent() / 100f },
                modifier = Modifier.fillMaxWidth(),
                color = if (t.state == "waiting") Pl.Warn else Pl.Accent,
                trackColor = Pl.Raised,
            )
        }
    }
}

@Composable
private fun WarnCard(text: String) {
    Panel(color = Pl.WarnSoft, border = Pl.Warn.copy(alpha = 0.4f)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Warning, contentDescription = null, tint = Pl.Warn)
            Spacer(Modifier.size(10.dp))
            Hint(text, color = Pl.Text)
        }
    }
}

@Composable
private fun NoteDialog(onSend: (String) -> Unit, onDismiss: () -> Unit) {
    var text by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Pl.Panel,
        title = { Text("Send a note") },
        text = {
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                placeholder = { Text("Shows as a popup on your computer") },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(onClick = { onSend(text.trim()) }, enabled = text.isNotBlank()) {
                Text("Send", color = Pl.AccentText)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = Pl.Muted) } },
        shape = RoundedCornerShape(16.dp),
    )
}
