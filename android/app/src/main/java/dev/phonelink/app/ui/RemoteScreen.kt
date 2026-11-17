package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phonelink.core.PresentKeyFfi

/** "My buttons": the computer's own buttons (made in its EchoTwin Settings → Phone buttons). */
@Composable
fun ButtonsScreen(state: UiState, actions: Actions, onClose: () -> Unit, modifier: Modifier = Modifier) {
    val computer = state.devices.firstOrNull { it.connected } ?: state.devices.firstOrNull()
    val name = computer?.name ?: "the computer"
    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onClose) { Text("‹ Back", color = Pl.AccentText) }
                Header("My buttons")
            }
        }
        if (computer?.connected != true) {
            item { Panel { Hint("$name is not connected. The buttons work while it is.") } }
            return@LazyColumn
        }
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
        item { Spacer(Modifier.height(16.dp)) }
    }
}

/** The presentation apps the Start key knows: id, label, start key, end key. */
private data class PresentApp(val id: String, val label: String, val start: PresentKeyFfi, val end: PresentKeyFfi)

private val PRESENT_APPS = listOf(
    PresentApp("office", "PowerPoint / Impress · F5", PresentKeyFfi.START, PresentKeyFfi.END),
    PresentApp("slides", "Google Slides · Ctrl+F5", PresentKeyFfi.START_SLIDES, PresentKeyFfi.END),
    PresentApp("canva", "Canva · Ctrl+Alt+P", PresentKeyFfi.START_CANVA, PresentKeyFfi.END),
    // F11 toggles full screen, so it ends it too.
    PresentApp("fullscreen", "Full screen · F11", PresentKeyFfi.START_FULLSCREEN, PresentKeyFfi.START_FULLSCREEN),
)

/**
 * Full-screen clicker: Previous and Next side by side (same size), one Start/End button for
 * the chosen app, and Black. The volume keys also work while it is open.
 */
@Composable
fun PresenterScreen(state: UiState, actions: Actions, onClose: () -> Unit, modifier: Modifier = Modifier) {
    val computer = state.devices.firstOrNull { it.connected } ?: state.devices.firstOrNull()
    // Older computers only know F5: no choice then.
    val choose = computer?.canPresentApps == true
    val app = PRESENT_APPS.firstOrNull { it.id == state.presentApp && choose } ?: PRESENT_APPS[0]
    // Starts as "not running" every time the screen opens.
    var running by remember { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    Column(modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onClose) { Text("‹ Done", color = Pl.AccentText) }
            Spacer(Modifier.weight(1f))
            if (choose) {
                Box {
                    Surface(
                        modifier = Modifier.clickable { menu = true },
                        color = Pl.Panel,
                        shape = RoundedCornerShape(50),
                        border = BorderStroke(1.dp, Pl.Edge),
                    ) {
                        Text(
                            app.label + "  ▾", fontSize = 13.sp, color = Pl.Text,
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 7.dp),
                        )
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        PRESENT_APPS.forEach { a ->
                            DropdownMenuItem(
                                text = { Text(a.label) },
                                onClick = {
                                    menu = false
                                    actions.setPresentApp(a.id)
                                },
                            )
                        }
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().height(170.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigKey("‹ Previous", Modifier.weight(1f).fillMaxHeight()) { actions.presentKey(PresentKeyFfi.PREVIOUS) }
            BigKey("Next ›", Modifier.weight(1f).fillMaxHeight(), accent = true) { actions.presentKey(PresentKeyFfi.NEXT) }
        }
        // The pointer pad goes here.
        Spacer(Modifier.weight(1f))
        Row(Modifier.fillMaxWidth().height(56.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigKey(
                if (running) "■ End" else "▶ Start",
                Modifier.weight(1f).fillMaxHeight(),
                textColor = if (running) Pl.Bad else Pl.Text,
                fontSize = 17,
            ) {
                actions.presentKey(if (running) app.end else app.start)
                running = !running
            }
            BigKey("Black", Modifier.weight(1f).fillMaxHeight(), fontSize = 17) { actions.presentKey(PresentKeyFfi.BLANK) }
        }
        Text(
            "Volume down = next · up = previous", fontSize = 12.sp, color = Pl.Muted,
            textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun BigKey(
    label: String,
    modifier: Modifier,
    accent: Boolean = false,
    textColor: Color? = null,
    fontSize: Int = 22,
    onClick: () -> Unit,
) {
    Box(
        modifier.fillMaxWidth()
            .background(if (accent) Pl.Accent else Pl.Panel, RoundedCornerShape(22.dp))
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, fontSize = fontSize.sp, fontWeight = FontWeight.SemiBold, color = textColor ?: if (accent) Color.White else Pl.Text)
    }
}

/** A round media key (also used by the music card on Home). */
@Composable
internal fun RoundKey(label: String, small: Boolean = false, accent: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
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
