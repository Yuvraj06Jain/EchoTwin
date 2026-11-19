package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.material3.Button
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
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
        else -> TouchpadPad(t, actions, modifier)
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
private fun TouchpadPad(t: TouchpadStatus, actions: Actions, modifier: Modifier) {
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

    val ready = t.connectedTo != null && t.padReady
    Column(
        modifier.fillMaxSize().padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        StatusLine(t, latency, actions)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            ClickButton("Left click", left = true, enabled = ready, actions = actions, modifier = Modifier.weight(1f))
            ClickButton("Right click", left = false, enabled = ready, actions = actions, modifier = Modifier.weight(1f))
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
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
                            sizeMm = w to h
                            actions.openPad(w, h)
                        },
                    ).also { padView = it }
                },
            )
            if (!ready) {
                Text(
                    when {
                        t.padError != null -> "The laptop couldn't make the touchpad. See the message above."
                        t.connectedTo == null -> "Not connected. Check that the laptop is on and the touchpad service is installed."
                        else -> "Connecting…"
                    },
                    color = Pl.Muted,
                    fontSize = 13.sp,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(32.dp),
                )
            }
        }
    }
}

@Composable
private fun StatusLine(t: TouchpadStatus, latency: Int?, actions: Actions) {
    val name = t.connectedTo ?: t.laptops.firstOrNull() ?: "Laptop"
    val (dot, detail, bad) = when {
        t.padError != null -> Triple(Pl.Bad, t.padError, true)
        t.connectedTo == null -> Triple(Pl.Warn, "looking for it…", false)
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
