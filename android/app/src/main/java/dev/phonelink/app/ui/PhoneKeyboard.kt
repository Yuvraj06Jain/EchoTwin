package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.phonelink.core.PadKey
import kotlinx.coroutines.delay

/**
 * The hidden field always holds this one character before what you type, so Backspace on an
 * "empty" field still reaches us (it deletes the sentinel) and is sent as Backspace.
 */
private const val SENTINEL = "​"

/** Clear the field after this many characters, at the next space, so it stays short. */
private const val RESET_AFTER = 40

private val CTRL = 1
private val ALT = 2
private val SUPER = 8

/**
 * Typing on the laptop, under the pad: a strip saying where you type (and what was sent
 * last), a row of special keys, and Android's own keyboard through a hidden field.
 *
 * What you type is compared with what was already sent: new characters go out, deletions
 * become Backspace, and a word the keyboard replaces (autocorrect) becomes Backspaces plus
 * the new word. Enter types Enter and starts afresh.
 */
@Composable
fun PhoneKeyboard(target: String, keyboardOff: Boolean, actions: Actions) {
    var field by remember { mutableStateOf(TextFieldValue(SENTINEL, TextRange(1))) }
    var sent by remember { mutableStateOf("") }
    var shown by remember { mutableStateOf("") } // the last characters sent, for the strip
    var note by remember { mutableStateOf<String?>(null) }
    var oneShot by remember { mutableIntStateOf(0) } // modifiers for the next key only
    var held by remember { mutableIntStateOf(0) } // modifiers held by a long press
    val focus = remember { FocusRequester() }
    val ime = LocalSoftwareKeyboardController.current

    LaunchedEffect(Unit) {
        focus.requestFocus()
        ime?.show()
    }
    DisposableEffect(Unit) { onDispose { ime?.hide() } }
    LaunchedEffect(note) {
        if (note != null) {
            delay(3500)
            note = null
        }
    }

    fun mods() = oneShot or held
    fun used() { oneShot = 0 }
    fun showSent(s: String) {
        shown = (shown + s.replace("\n", "⏎")).takeLast(24)
    }

    fun onChange(v: TextFieldValue) {
        val sentinelGone = !v.text.startsWith(SENTINEL)
        val now = if (sentinelGone) v.text else v.text.removePrefix(SENTINEL)
        var common = 0
        while (common < sent.length && common < now.length && sent[common] == now[common]) common++
        val backspaces = (sent.length - common) + if (sentinelGone) 1 else 0
        val added = now.substring(common)
        repeat(backspaces) { actions.key(PadKey.Backspace, mods()) }
        if (backspaces > 0) shown = shown.dropLast(backspaces)
        if (added.isNotEmpty()) {
            if (mods() != 0 && added.length == 1 && added != "\n") {
                // Ctrl (or Alt, Super) armed: a shortcut, not text.
                actions.key(PadKey.Char(added.lowercase()), mods())
                used()
                field = TextFieldValue(SENTINEL, TextRange(1))
                sent = ""
                return
            }
            actions.typeText(added) { note = it }
            showSent(added)
        }
        // Start afresh after Enter, a lost sentinel, or once it gets long (at a space).
        val restart = sentinelGone || added.contains('\n') || (now.length > RESET_AFTER && now.endsWith(' '))
        if (restart) {
            field = TextFieldValue(SENTINEL, TextRange(1))
            sent = ""
        } else {
            field = v
            sent = now
        }
    }

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Surface(
            modifier = Modifier.fillMaxWidth().combinedClickable(onClick = { focus.requestFocus(); ime?.show() }),
            shape = RoundedCornerShape(12.dp),
            color = Pl.Panel,
            border = BorderStroke(1.dp, Pl.Edge),
        ) {
            Row(Modifier.padding(horizontal = 10.dp, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                val text = when {
                    keyboardOff -> "Typing is switched off on the laptop (EchoTwin → Settings → Touchpad)"
                    note != null -> note!!
                    else -> null
                }
                if (text != null) {
                    Text(text, color = if (keyboardOff) Pl.Bad else Pl.Warn, fontSize = 12.sp, maxLines = 2)
                } else {
                    Text("Typing to ", color = Pl.Muted, fontSize = 12.sp)
                    Text(target, fontWeight = FontWeight.SemiBold, fontSize = 12.sp, maxLines = 1)
                    Text(
                        if (shown.isEmpty()) "" else "  …$shown",
                        color = Pl.AccentText,
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        SpecialKeys(
            armed = oneShot,
            held = held,
            onModifier = { bit -> oneShot = oneShot xor bit },
            onHold = { bit -> held = held xor bit; oneShot = oneShot and bit.inv() },
            onKey = { k -> actions.key(k, mods()); used() },
        )
        // The hidden field Android's keyboard types into. Text, not a password field, so the
        // keyboard keeps its emoji key; autocorrect is off and anything it replaces is undone
        // by the comparison above.
        BasicTextField(
            value = field,
            onValueChange = ::onChange,
            modifier = Modifier.size(1.dp).alpha(0f).focusRequester(focus),
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Text,
                imeAction = ImeAction.None,
            ),
            singleLine = false,
        )
    }
}

/** Esc, Tab, Ctrl, Alt, Super, the arrows, Home, End, Del; scrolls sideways. */
@Composable
private fun SpecialKeys(
    armed: Int,
    held: Int,
    onModifier: (Int) -> Unit,
    onHold: (Int) -> Unit,
    onKey: (PadKey) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        SpecialKey("Esc") { onKey(PadKey.Escape) }
        SpecialKey("Tab") { onKey(PadKey.Tab) }
        for ((label, bit) in listOf("Ctrl" to CTRL, "Alt" to ALT, "Super" to SUPER)) {
            SpecialKey(
                label,
                lit = armed and bit != 0 || held and bit != 0,
                locked = held and bit != 0,
                onLongClick = { onHold(bit) },
            ) {
                if (held and bit != 0) onHold(bit) else onModifier(bit)
            }
        }
        SpecialKey("←") { onKey(PadKey.Left) }
        SpecialKey("↑") { onKey(PadKey.Up) }
        SpecialKey("↓") { onKey(PadKey.Down) }
        SpecialKey("→") { onKey(PadKey.Right) }
        SpecialKey("Home") { onKey(PadKey.Home) }
        SpecialKey("End") { onKey(PadKey.End) }
        SpecialKey("Del") { onKey(PadKey.Delete) }
    }
}

@Composable
private fun SpecialKey(
    label: String,
    lit: Boolean = false,
    locked: Boolean = false,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .height(34.dp)
            .widthIn(min = 40.dp)
            .background(if (lit) Pl.AccentSoft else Pl.Raised, RoundedCornerShape(8.dp))
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            if (locked) "$label •" else label,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (lit) Pl.AccentText else Pl.Text,
        )
    }
}
