package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.foundation.layout.imePadding
import androidx.compose.material3.Button
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.ui.platform.LocalDensity
import kotlin.math.abs
import kotlin.math.floor
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.phonelink.app.TouchPadView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** The pad's surface: a little darker than the panels around it. */
private val PadSurface = Color(0xFF15181B)

/**
 * The Touchpad tab: the phone as the laptop's touchpad. It has its own pairing with the
 * laptop's touchpad service (`phonelink touchpad pair`), separate from the main link.
 */
@Composable
fun TouchpadScreen(state: UiState, actions: Actions, modifier: Modifier = Modifier) {
    LaunchedEffect(Unit) { dev.phonelink.app.LinkManager.startTouchpad() }
    val t = state.touchpad
    when {
        // A moment while its keys load: no flash of the setup card on a paired phone.
        !t.started && t.startError == null -> Box(modifier.fillMaxSize())
        !t.paired -> TouchpadSetup(t, actions, modifier)
        else -> TouchpadPad(t, state.padPrefs, actions, modifier)
    }
}

@Composable
private fun TouchpadSetup(t: TouchpadStatus, actions: Actions, modifier: Modifier) {
    Column(
        modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Header("Touchpad")
        Panel {
            Text("Use your phone as the laptop's touchpad", fontWeight = FontWeight.SemiBold)
            Hint("One-time setup. On the laptop run")
            Text("phonelink touchpad pair", fontFamily = FontFamily.Monospace, fontSize = 13.sp, color = Pl.Text)
            Hint("approve the admin prompt, then scan the code it shows.")
            Button(
                onClick = actions::scanTouchpad,
                enabled = t.pairing == null,
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(10.dp),
            ) { Text(t.pairing ?: "Scan") }
            t.startError?.let { Hint("The touchpad couldn't start: $it", color = Pl.Bad) }
        }
        Panel {
            Hint(
                "Moves the pointer, tap to click, two fingers to scroll, three-finger swipes like your real " +
                    "touchpad. Plus Left and Right click buttons.",
            )
            Hint(
                "The touchpad has its own pairing with its own secure service on the laptop " +
                    "(install it once with packaging/install-touchpad.sh).",
            )
        }
    }
}

@Composable
private fun TouchpadPad(t: TouchpadStatus, prefs: PadPrefs, actions: Actions, modifier: Modifier) {
    // Keep the screen on while the pad is shown.
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }

    var sizeMm by remember { mutableStateOf<Pair<Float, Float>?>(null) }
    var padView by remember { mutableStateOf<TouchPadView?>(null) }
    val currentSize by rememberUpdatedState(sizeMm)
    // Leaving the tab or the app lifts every finger and button on the laptop; coming back
    // opens the pad again.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, e ->
            when (e) {
                Lifecycle.Event.ON_START -> currentSize?.let { (w, h) -> actions.openPad(w, h) }
                Lifecycle.Event.ON_STOP -> {
                    padView?.liftAll()
                    actions.closePad()
                }
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            padView?.liftAll()
            actions.closePad()
        }
    }

    var latency by remember { mutableStateOf<Int?>(null) }
    LaunchedEffect(t.connectedTo, t.padReady) {
        latency = null
        while (t.connectedTo != null && t.padReady) {
            latency = withContext(Dispatchers.IO) { actions.touchpadLatency() }
            delay(1000)
        }
    }

    val connectedAndReady = t.connectedTo != null && t.padReady
    val ready = connectedAndReady && !t.laptopInUse
    var settingsOpen by remember { mutableStateOf(false) }
    var typing by remember { mutableStateOf(false) }
    val typingNow by rememberUpdatedState(typing)
    Column(
        modifier.fillMaxSize().imePadding().padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        StatusLine(t, latency, actions, onSettings = { settingsOpen = true })
        // Left click, the scroll strip and Right click, in the order chosen in the settings.
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            for (item in prefs.layout) {
                when (item) {
                    'L' -> ClickButton("Left click", left = true, enabled = ready, actions = actions, modifier = Modifier.weight(1f))
                    'R' -> ClickButton("Right click", left = false, enabled = ready, actions = actions, modifier = Modifier.weight(1f))
                    else -> ScrollStrip(enabled = ready, prefs = prefs, actions = actions, modifier = Modifier.width(74.dp))
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
        Box(
            Modifier
                .fillMaxSize()
                .alpha(if (t.laptopInUse) 0.4f else 1f)
                .clip(RoundedCornerShape(20.dp))
                .background(PadSurface)
                .drawBehind {
                    val stroke = Stroke(
                        width = 2.dp.toPx(),
                        pathEffect = PathEffect.dashPathEffect(floatArrayOf(14f, 10f)),
                    )
                    drawRoundRect(
                        Pl.Accent.copy(alpha = if (ready) 0.7f else 0.25f),
                        cornerRadius = CornerRadius(20.dp.toPx()),
                        style = stroke,
                    )
                },
            contentAlignment = Alignment.Center,
        ) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    TouchPadView(
                        ctx,
                        onFingers = { actions.touch(it) },
                        onSize = { w, h ->
                            // While typing, the keyboard squeezes the pad: keep the size the
                            // laptop knows, so it doesn't rebuild its touchpad each time.
                            if (!typingNow || sizeMm == null) {
                                sizeMm = w to h
                                actions.openPad(w, h)
                            }
                        },
                    ).also { padView = it }
                },
            )
            if (!ready) {
                Text(
                    when {
                        t.padError != null -> "The laptop can't use the touchpad right now. See the message above."
                        t.connectedTo == null -> "Not connected. Check that the laptop is on and the touchpad service is installed."
                        t.laptopInUse -> "Your laptop's own touchpad is being used. The phone takes over again a moment after you let go of it."
                        else -> "Connecting…"
                    },
                    color = Pl.Muted,
                    fontSize = 13.sp,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(32.dp),
                )
            }
        }
            // The keyboard button (✕ while typing), over the pad's corner.
            Surface(
                onClick = { typing = !typing },
                modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp).size(48.dp),
                shape = CircleShape,
                color = Pl.Accent,
                shadowElevation = 6.dp,
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Icon(
                        if (typing) Icons.Filled.Close else Glyphs.Keyboard,
                        contentDescription = if (typing) "Close the keyboard" else "Type on the laptop",
                        tint = Color.White,
                    )
                }
            }
        }
        if (typing) {
            PhoneKeyboard(t.connectedTo ?: t.laptops.firstOrNull() ?: "the laptop", t.keyboardOff, actions)
        }
    }
    if (settingsOpen) TouchpadSettingsDialog(prefs, actions) { settingsOpen = false }
}

@Composable
private fun TouchpadSettingsDialog(prefs: PadPrefs, actions: Actions, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Pl.Panel,
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done", color = Pl.AccentText) } },
        title = { Text("Touchpad") },
        text = { TouchpadSettings(prefs, actions, forget = null) },
    )
}

@Composable
private fun StatusLine(t: TouchpadStatus, latency: Int?, actions: Actions, onSettings: () -> Unit) {
    val name = t.connectedTo ?: t.laptops.firstOrNull() ?: "Laptop"
    val (dot, detail, bad) = when {
        t.padError != null -> Triple(Pl.Bad, t.padError, true)
        t.connectedTo == null -> Triple(Pl.Warn, "looking for it…", false)
        t.laptopInUse -> Triple(Pl.Warn, "its touchpad is in use", false)
        !t.padReady -> Triple(Pl.Warn, "connecting…", false)
        else -> Triple(Pl.AccentText, "connected" + (latency?.let { " · $it ms" } ?: ""), false)
    }
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(8.dp).background(dot, CircleShape))
        Spacer(Modifier.width(8.dp))
        Text(name, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(
            " · $detail",
            color = if (bad) Pl.Bad else Pl.Muted,
            fontSize = 13.sp,
            maxLines = 3,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Box {
            TextButton(onClick = { menu = true }) { Text("⋯", color = Pl.Muted, fontSize = 18.sp) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(text = { Text("Layout and speed…") }, onClick = { menu = false; onSettings() })
                DropdownMenuItem(text = { Text("Pair another laptop") }, onClick = { menu = false; actions.scanTouchpad() })
                DropdownMenuItem(text = { Text("Forget the touchpad pairing") }, onClick = { menu = false; actions.forgetTouchpad() })
            }
        }
    }
}

/** Left click / Right click: held while the finger is on it (hold Left and move a finger to drag). */
@Composable
private fun ClickButton(label: String, left: Boolean, enabled: Boolean, actions: Actions, modifier: Modifier) {
    var pressed by remember { mutableStateOf(false) }
    val haptic = LocalHapticFeedback.current
    Surface(
        modifier = modifier
            .height(72.dp)
            .alpha(if (enabled) 1f else 0.45f)
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false).consume()
                    pressed = true
                    haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                    actions.button(left, true)
                    try {
                        waitForUpOrCancellation()
                    } finally {
                        pressed = false
                        actions.button(left, false)
                    }
                }
            },
        shape = RoundedCornerShape(16.dp),
        color = if (pressed) Pl.AccentSoft else Pl.Panel,
        border = BorderStroke(1.dp, if (pressed) Pl.Accent else Pl.Edge),
    ) {
        Box(contentAlignment = Alignment.Center) {
            Text(label, fontWeight = FontWeight.SemiBold, color = if (pressed) Pl.AccentText else Pl.Text)
        }
    }
}

/**
 * The scroll strip: drag up or down to scroll, like a mouse wheel. Each millimetre of drag
 * is about 20 hi-res units (a notch every 6 mm) times the scroll speed; sent at most every
 * 16 ms, with a light tick at every notch.
 */
@Composable
private fun ScrollStrip(enabled: Boolean, prefs: PadPrefs, actions: Actions, modifier: Modifier) {
    var dragging by remember { mutableStateOf(0) } // -1 down, 0 idle, 1 up
    val haptic = LocalHapticFeedback.current
    val pxPerMm = with(LocalDensity.current) { 1.dp.toPx() * 160f / 25.4f }
    val current by rememberUpdatedState(prefs)
    val (base, rib) = if (dragging != 0) Color(0xFF1B3328) to Color(0xFF21402F) else Pl.Panel to Color(0xFF20252A)
    Surface(
        modifier = modifier
            .height(72.dp)
            .alpha(if (enabled) 1f else 0.45f)
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    var lastY = down.position.y
                    var pending = 0f // v120 not sent yet
                    var total = 0f // v120 this drag, for the notch ticks
                    var lastSent = 0L
                    try {
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                            if (!change.pressed) break
                            val dy = change.position.y - lastY
                            lastY = change.position.y
                            change.consume()
                            if (dy == 0f) continue
                            val sign = if (current.reverseScroll) 1f else -1f // finger up = scroll up
                            val v = sign * dy / pxPerMm * 20f * current.scrollSpeed
                            dragging = if (v > 0) 1 else -1
                            val before = floor(total / 120f)
                            total += v
                            pending += v
                            if (floor(total / 120f) != before) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            if (change.uptimeMillis - lastSent >= 16 && abs(pending) >= 1f) {
                                val send = pending.toInt()
                                pending -= send
                                lastSent = change.uptimeMillis
                                actions.scroll(send)
                            }
                        }
                        if (abs(pending) >= 1f) actions.scroll(pending.toInt())
                    } finally {
                        dragging = 0
                    }
                }
            },
        shape = RoundedCornerShape(16.dp),
        color = Color.Transparent,
        border = BorderStroke(1.dp, if (dragging != 0) Pl.Accent else Pl.Edge),
    ) {
        Box(
            Modifier
                .fillMaxSize()
                .background(base)
                .drawBehind { ribs(rib, 2.dp.toPx(), 7.dp.toPx()) },
            contentAlignment = Alignment.Center,
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("▲", fontSize = 10.sp, color = if (dragging == 1) Pl.AccentText else Pl.Muted)
                Text("Scroll", fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = if (dragging != 0) Pl.AccentText else Pl.Muted)
                Text("▼", fontSize = 10.sp, color = if (dragging == -1) Pl.AccentText else Pl.Muted)
            }
        }
    }
}

/**
 * The Touchpad's settings: layout, pointer and scroll speed, reverse scrolling, and
 * (in the Settings tab) forgetting the pairing. Changes apply at once.
 */
@Composable
fun TouchpadSettings(prefs: PadPrefs, actions: Actions, forget: (() -> Unit)?) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Layout", fontWeight = FontWeight.Medium)
        Hint("Order of the buttons above the pad: L = Left click, R = Right click, ribbed = scroll.")
        for (row in PadPrefs.LAYOUTS.chunked(3)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (layout in row) {
                    LayoutChoice(layout, chosen = layout == prefs.layout, modifier = Modifier.weight(1f)) {
                        actions.setPadPrefs(prefs.copy(layout = layout))
                    }
                }
            }
        }
        SpeedSlider("Pointer speed", prefs.pointerSpeed, 0.5f..2f, steps = 5) {
            actions.setPadPrefs(prefs.copy(pointerSpeed = it))
        }
        SpeedSlider("Scroll speed", prefs.scrollSpeed, 0.5f..3f, steps = 9) {
            actions.setPadPrefs(prefs.copy(scrollSpeed = it))
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Reverse scroll direction", fontWeight = FontWeight.Medium)
                Hint("Drag up to scroll down")
            }
            Switch(
                checked = prefs.reverseScroll,
                onCheckedChange = { actions.setPadPrefs(prefs.copy(reverseScroll = it)) },
                colors = SwitchDefaults.colors(checkedTrackColor = Pl.Accent),
            )
        }
        forget?.let {
            TextButton(onClick = it) { Text("Forget the touchpad pairing", color = Pl.Bad) }
        }
    }
}

@Composable
private fun SpeedSlider(label: String, value: Float, range: ClosedFloatingPointRange<Float>, steps: Int, onChange: (Float) -> Unit) {
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(label, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
            Text(formatSpeed(value), color = Pl.AccentText, fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
        }
        Slider(
            value = value,
            onValueChange = onChange,
            valueRange = range,
            steps = steps,
            colors = SliderDefaults.colors(thumbColor = Pl.AccentText, activeTrackColor = Pl.AccentText),
        )
    }
}

/** "1×", "1.25×". */
fun formatSpeed(v: Float): String {
    val r = Math.round(v * 100) / 100f
    return (if (r == r.toInt().toFloat()) r.toInt().toString() else r.toString()) + "×"
}

/** A small drawing of one order: L and R as keys, the scroll strip ribbed. */
@Composable
private fun LayoutChoice(layout: String, chosen: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val key = if (chosen) Color(0xFF24543C) else Color(0xFF2A2F35)
    val rib = if (chosen) Color(0xFF2E6A4B) else Color(0xFF353B42)
    Row(
        modifier
            .clip(RoundedCornerShape(10.dp))
            .background(if (chosen) Pl.AccentSoft else Color(0xFF15181B))
            .border(BorderStroke(1.dp, if (chosen) Pl.Accent else Pl.Edge), RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(6.dp),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        for (c in layout) {
            if (c == 'S') {
                Box(
                    Modifier.width(14.dp).height(20.dp).clip(RoundedCornerShape(5.dp))
                        .background(key)
                        .drawBehind { ribs(rib, 1.dp.toPx(), 4.dp.toPx()) },
                )
            } else {
                Box(
                    Modifier.weight(1f).height(20.dp).clip(RoundedCornerShape(5.dp)).background(key),
                    contentAlignment = Alignment.Center,
                ) { Text(c.toString(), fontSize = 9.sp, color = if (chosen) Color(0xFFCFEEE0) else Pl.Muted) }
            }
        }
    }
}

/** Horizontal ribs across the area, like a mouse wheel's grip. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.ribs(color: Color, thick: Float, every: Float) {
    var y = every / 2
    while (y < size.height) {
        drawRect(color, topLeft = Offset(0f, y), size = Size(size.width, thick))
        y += every
    }
}
