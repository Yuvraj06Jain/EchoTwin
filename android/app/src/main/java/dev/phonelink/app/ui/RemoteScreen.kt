package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phonelink.core.MediaCommand
import dev.phonelink.core.PresentKeyFfi

/** The phone as a remote for the computer: what plays there, your own buttons, slides. */
@Composable
fun RemoteScreen(state: UiState, actions: Actions, onClose: () -> Unit, modifier: Modifier = Modifier) {
    val computer = state.devices.firstOrNull { it.connected } ?: state.devices.firstOrNull()
    val name = computer?.name ?: "the computer"
    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onClose) { Text("‹ Back", color = Pl.AccentText) }
                Header("Remote")
            }
        }
        if (computer?.connected != true) {
            item { Panel { Hint("$name is not connected. The remote works while it is.") } }
            return@LazyColumn
        }
        item { SectionTitle("Playing on $name") }
        item {
            val m = state.computerMedia
            Panel {
                if (m == null) {
                    Hint("Nothing is playing on $name.")
                } else {
                    Text(m.title.ifBlank { m.app }, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Hint(listOf(m.artist, m.app).filter { it.isNotBlank() }.joinToString(" · "))
                    Row(
                        Modifier.fillMaxWidth().padding(top = 6.dp),
                        horizontalArrangement = Arrangement.SpaceEvenly,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RoundKey("−", small = true) { actions.computerMedia(MediaCommand.VolumeDown) }
                        RoundKey("⏮", small = true, enabled = m.canPrevious) { actions.computerMedia(MediaCommand.Previous) }
                        RoundKey(if (m.playing) "❚❚" else "▶", accent = true) { actions.computerMedia(MediaCommand.PlayPause) }
                        RoundKey("⏭", small = true, enabled = m.canNext) { actions.computerMedia(MediaCommand.Next) }
                        RoundKey("+", small = true) { actions.computerMedia(MediaCommand.VolumeUp) }
                    }
                }
            }
        }
        item { SectionTitle("Buttons") }
        item {
            Panel {
                if (state.commands.isEmpty()) {
                    Hint("Add your own buttons on the computer: EchoTwin → Settings → Phone buttons.")
                } else {
                    state.commands.chunked(2).forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            row.forEach { b ->
                                Surface(
                                    modifier = Modifier.weight(1f).clickable { actions.runCommand(b.id) },
                                    color = Pl.Raised,
                                    shape = RoundedCornerShape(14.dp),
                                    border = BorderStroke(1.dp, Pl.Edge),
                                ) {
                                    Text(
                                        b.name, modifier = Modifier.padding(vertical = 14.dp, horizontal = 10.dp),
                                        textAlign = TextAlign.Center, maxLines = 2, overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                            if (row.size == 1) Spacer(Modifier.weight(1f))
                        }
                    }
                }
            }
        }
        if (computer?.canPresent == true) {
            item { SectionTitle("Presentation") }
            item {
                Panel {
                    Hint("Next and previous slide, start, end and a black screen. The volume keys work as a clicker while it is open.")
                    TextButton(onClick = actions::openPresenter) { Text("Start presenting", color = Pl.AccentText) }
                }
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }
}

/** Full-screen clicker: big Next / Previous, small Start / End / Black screen. */
@Composable
fun PresenterScreen(actions: Actions, onClose: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onClose) { Text("‹ Done", color = Pl.AccentText) }
            Spacer(Modifier.weight(1f))
            Hint("Volume down = next · up = previous")
        }
        BigKey("Next ›", Modifier.weight(2f), accent = true) { actions.presentKey(PresentKeyFfi.NEXT) }
        BigKey("‹ Previous", Modifier.weight(1f)) { actions.presentKey(PresentKeyFfi.PREVIOUS) }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigKey("Start", Modifier.weight(1f).height(64.dp)) { actions.presentKey(PresentKeyFfi.START) }
            BigKey("Black", Modifier.weight(1f).height(64.dp)) { actions.presentKey(PresentKeyFfi.BLANK) }
            BigKey("End", Modifier.weight(1f).height(64.dp)) { actions.presentKey(PresentKeyFfi.END) }
        }
    }
}

@Composable
private fun BigKey(label: String, modifier: Modifier, accent: Boolean = false, onClick: () -> Unit) {
    Box(
        modifier.fillMaxWidth()
            .background(if (accent) Pl.Accent else Pl.Panel, RoundedCornerShape(22.dp))
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, fontSize = 22.sp, fontWeight = FontWeight.SemiBold, color = if (accent) Color.White else Pl.Text)
    }
}

@Composable
private fun RoundKey(label: String, small: Boolean = false, accent: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val size = if (small) 44.dp else 60.dp
    Box(
        Modifier.size(size)
            .background(if (accent) Pl.Accent else Pl.Raised, CircleShape)
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label, fontSize = if (small) 16.sp else 20.sp,
            color = when {
                !enabled -> Pl.Muted.copy(alpha = 0.4f)
                accent -> Color.White
                else -> Pl.Text
            },
        )
    }
}
