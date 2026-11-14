package dev.phonelink.app.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** How the two devices are linked right now, for the pill between them. */
enum class Link(val label: String) {
    Wifi("Wi-Fi"),
    Bluetooth("Bluetooth"),
    Off("Off"),
    Offline("Offline"),
    PausedThere("Off on the computer"),
}

/** Amber under 20% when not charging, else the link colour. */
private fun levelColor(level: Int?, charging: Boolean, live: Boolean): Color = when {
    !live || level == null -> Pl.Muted
    level < 20 && !charging -> Pl.Warn
    else -> Pl.AccentText
}

/**
 * "Twin link": this phone and the computer drawn side by side, each filled to its battery
 * level, with the link between them (dashes that drift while connected).
 */
@Composable
fun TwinHeader(
    computer: String,
    computerBattery: Int?,
    computerCharging: Boolean,
    myBattery: Int?,
    myCharging: Boolean,
    myNetwork: String,
    link: Link,
) {
    val live = link == Link.Wifi || link == Link.Bluetooth
    val linkColor = when (link) {
        Link.Wifi -> Pl.AccentText
        Link.Bluetooth -> Pl.Warn
        else -> Pl.Muted
    }
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
        color = Pl.Panel,
        border = BorderStroke(1.dp, Pl.AccentText.copy(alpha = if (live) 0.25f else 0.08f)),
    ) {
        Box(
            Modifier.background(
                Brush.verticalGradient(
                    listOf(Pl.AccentText.copy(alpha = if (live) 0.10f else 0.03f), Color.Transparent)
                )
            ).padding(horizontal = 14.dp, vertical = 18.dp)
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Device(
                    Modifier.weight(1f),
                    title = "This phone",
                    detail = listOfNotNull(myBattery?.let { "$it%" }, myNetwork.takeIf { it.isNotEmpty() })
                        .joinToString(" · ").ifEmpty { "–" },
                ) {
                    PhoneOutline(myBattery, levelColor(myBattery, myCharging, true))
                }
                LinkLine(Modifier.weight(1.1f), link, linkColor, live)
                Device(
                    Modifier.weight(1f),
                    title = computer,
                    detail = when {
                        !live -> link.label
                        computerBattery == null -> "Connected"
                        else -> "$computerBattery%" + if (computerCharging) " · charging" else ""
                    },
                ) {
                    LaptopOutline(if (live) computerBattery else null, levelColor(computerBattery, computerCharging, live))
                }
            }
        }
    }
}

@Composable
private fun Device(modifier: Modifier, title: String, detail: String, art: @Composable () -> Unit) {
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.height(78.dp), contentAlignment = Alignment.Center) { art() }
        Spacer(Modifier.height(10.dp))
        Text(
            title, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, maxLines = 1,
            overflow = TextOverflow.Ellipsis, textAlign = TextAlign.Center,
        )
        Text(detail, color = Pl.Muted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A filled-to-level rounded body; `level` null = outline only. */
private fun DrawScope.levelBody(topLeft: Offset, size: Size, radius: Float, level: Int?, color: Color) {
    val stroke = 2.dp.toPx()
    drawRoundRect(color, topLeft, size, CornerRadius(radius), style = Stroke(stroke))
    if (level == null) return
    val inset = stroke + 2.dp.toPx()
    val inner = Size(size.width - inset * 2, size.height - inset * 2)
    val fillTop = topLeft.y + inset + inner.height * (1 - level.coerceIn(0, 100) / 100f)
    clipRect(topLeft.x, fillTop, topLeft.x + size.width, topLeft.y + size.height) {
        drawRoundRect(
            color.copy(alpha = 0.75f),
            Offset(topLeft.x + inset, topLeft.y + inset),
            inner,
            CornerRadius((radius - inset).coerceAtLeast(2f)),
        )
    }
}

@Composable
private fun PhoneOutline(level: Int?, color: Color) {
    Canvas(Modifier.size(width = 42.dp, height = 74.dp)) {
        levelBody(Offset.Zero, size, 9.dp.toPx(), level, color)
        // Camera.
        drawCircle(color, radius = 2.dp.toPx(), center = Offset(size.width / 2, 6.dp.toPx()))
    }
}

@Composable
private fun LaptopOutline(level: Int?, color: Color) {
    Canvas(Modifier.size(width = 96.dp, height = 64.dp)) {
        val screen = Size(size.width * 0.82f, size.height * 0.80f)
        levelBody(Offset((size.width - screen.width) / 2, 0f), screen, 6.dp.toPx(), level, color)
        // The base: a slim bar a little wider than the screen.
        drawRoundRect(
            color,
            Offset(0f, size.height - 6.dp.toPx()),
            Size(size.width, 4.dp.toPx()),
            CornerRadius(2.dp.toPx()),
        )
    }
}

@Composable
private fun LinkLine(modifier: Modifier, link: Link, color: Color, live: Boolean) {
    // The dashes drift only while connected and only while this screen is shown.
    val phase = if (live) {
        val t = rememberInfiniteTransition(label = "link")
        val p by t.animateFloat(
            initialValue = 0f, targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(1400, easing = LinearEasing), RepeatMode.Restart),
            label = "dash",
        )
        p
    } else 0f
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Box(Modifier.height(78.dp).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Canvas(Modifier.fillMaxWidth().height(2.dp)) {
                val dash = 6.dp.toPx()
                val gap = 5.dp.toPx()
                drawLine(
                    color.copy(alpha = if (live) 0.9f else 0.4f),
                    Offset(0f, size.height / 2),
                    Offset(size.width, size.height / 2),
                    strokeWidth = 2.dp.toPx(),
                    pathEffect = PathEffect.dashPathEffect(floatArrayOf(dash, gap), -phase * (dash + gap)),
                )
            }
            Row(
                Modifier.background(Pl.Bg, CircleShape)
                    .background(color.copy(alpha = 0.16f), CircleShape)
                    .padding(horizontal = 9.dp, vertical = 3.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(6.dp).background(color, CircleShape))
                Spacer(Modifier.width(5.dp))
                Text(link.label, color = color, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            }
        }
        Spacer(Modifier.height(10.dp))
        Text(" ", fontSize = 14.sp)
        Text(" ", fontSize = 12.sp)
    }
}
