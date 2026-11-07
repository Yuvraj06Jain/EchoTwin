package dev.phonelink.app.ui

import androidx.compose.foundation.clickable
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
        item { Header("PhoneLink") }

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
            item { ComputerCard(main, state, actions, onNote = { writingNote = true }) }
            if (main.connected && main.via == "bluetooth") {
                item {
                    WarnCard(
                        "Using the Bluetooth backup. Notifications, messages and the clipboard still " +
                            "work; files wait for Wi-Fi."
                    )
                }
            }
        }

        if (active.isNotEmpty()) {
            item { SectionTitle("Now") }
            items(active.take(3), key = { "now-" + it.id }) { TransferCard(it, actions) }
        }

        item { SectionTitle("On your computer") }
        item {
            Panel {
                SwitchRow(
                    "Notifications",
                    if (state.access.notifications) "Shown as popups and in the PhoneLink window"
                    else "Needs notification access",
                    checked = state.access.notifications,
                ) { actions.allowNotifications() }
                SwitchRow(
                    "Messages (SMS)",
                    if (state.access.sms) "Read and answer texts from the computer" else "Needs SMS access",
                    checked = state.access.sms,
                ) { if (it) actions.allowSms() else actions.openAppInfo() }
                SwitchRow(
                    "Share clipboard",
                    "Copy on the computer, paste here",
                    checked = state.clipboardOn,
                ) { actions.setClipboard(it) }
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

fun statusLabel(d: Device): String = when {
    d.connected && d.via == "bluetooth" -> "Connected · Bluetooth"
    d.connected -> "Connected · Wi-Fi"
    else -> "Offline"
}

@Composable
private fun ComputerCard(d: Device, state: UiState, actions: Actions, onNote: () -> Unit) {
    val bluetooth = d.connected && d.via == "bluetooth"
    Panel {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(d.name, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.height(6.dp))
                Chip(
                    statusLabel(d),
                    when {
                        bluetooth -> Pl.Warn
                        d.connected -> Pl.AccentText
                        else -> Pl.Muted
                    },
                )
            }
            if (!d.connected) TextButton(onClick = actions::reconnect) { Text("Reconnect", color = Pl.AccentText) }
        }
        when {
            bluetooth -> Hint("Wi-Fi can't reach the computer right now.")
            !d.connected -> Hint(
                (state.bluetoothProblem?.let { "$it. " } ?: "") +
                    "Make sure both are on the same Wi-Fi (or your hotspot)." +
                    if (d.pending > 0uL) " ${d.pending} waiting to send." else ""
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ActionTile("Lock", Icons.Filled.Lock, d.canLock, Modifier.weight(1f), actions::lock)
            ActionTile(
                "Clipboard", Icons.Filled.Share, d.canClipboard && state.clipboardOn,
                Modifier.weight(1f), actions::sendClipboard,
            )
            ActionTile(
                "Send files", Icons.AutoMirrored.Filled.Send, !bluetooth,
                Modifier.weight(1f), actions::sendFiles,
            )
            ActionTile("Note", Icons.Filled.Edit, true, Modifier.weight(1f), onNote)
        }
    }
}

@Composable
private fun ActionTile(label: String, icon: ImageVector, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val tint = if (enabled) Pl.AccentText else Pl.Muted.copy(alpha = 0.5f)
    Surface(
        modifier = modifier.clickable(enabled = enabled, onClick = onClick),
        color = Pl.Raised,
        shape = RoundedCornerShape(12.dp),
    ) {
        Column(
            Modifier.padding(vertical = 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
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
