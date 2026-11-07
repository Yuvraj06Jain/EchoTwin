package dev.phonelink.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** The same calm dark palette as the laptop app. */
object Pl {
    val Bg = Color(0xFF16181B)
    val Panel = Color(0xFF1D2024)
    val Raised = Color(0xFF25292E)
    val Line = Color(0xFF2A2E33)
    val Text = Color(0xFFE6E8EA)
    val Muted = Color(0xFF9AA0A6)
    val Accent = Color(0xFF23905F)
    val AccentText = Color(0xFF3DBB82)
    val AccentSoft = Color(0xFF1B3328)
    val Warn = Color(0xFFE0B341)
    val WarnSoft = Color(0xFF332B17)
    val Bad = Color(0xFFE5707A)
}

@Composable
fun PhoneLinkTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Pl.Accent,
            onPrimary = Color.White,
            secondaryContainer = Pl.AccentSoft,
            onSecondaryContainer = Pl.AccentText,
            background = Pl.Bg,
            onBackground = Pl.Text,
            surface = Pl.Bg,
            onSurface = Pl.Text,
            surfaceVariant = Pl.Panel,
            onSurfaceVariant = Pl.Muted,
            surfaceContainer = Pl.Panel,
            surfaceContainerHigh = Pl.Raised,
            surfaceContainerHighest = Pl.Raised,
            outline = Pl.Line,
            outlineVariant = Pl.Line,
            error = Pl.Bad,
        ),
        content = content,
    )
}

/** A rounded panel with a hairline border. */
@Composable
fun Panel(
    modifier: Modifier = Modifier,
    color: Color = Pl.Panel,
    border: Color = Pl.Line,
    content: @Composable ColumnScope.() -> Unit,
) {
    Surface(
        modifier = modifier.fillMaxWidth(),
        color = color,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, border),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
    }
}

@Composable
fun SectionTitle(text: String) {
    Text(
        text.uppercase(),
        color = Pl.Muted,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.6.sp,
        modifier = Modifier.padding(top = 6.dp, start = 4.dp),
    )
}

@Composable
fun Hint(text: String, color: Color = Pl.Muted) {
    Text(text, color = color, fontSize = 13.sp, lineHeight = 18.sp)
}

/** A small status pill with a dot, e.g. "Connected · Wi-Fi". */
@Composable
fun Chip(text: String, color: Color) {
    Row(
        Modifier.background(color.copy(alpha = 0.14f), CircleShape).padding(horizontal = 10.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(7.dp).background(color, CircleShape))
        Spacer(Modifier.width(6.dp))
        Text(text, color = color, fontSize = 12.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
fun SwitchRow(title: String, subtitle: String, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, color = if (enabled) Pl.Text else Pl.Muted, fontWeight = FontWeight.Medium)
            Hint(subtitle)
        }
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            enabled = enabled,
            colors = SwitchDefaults.colors(
                checkedTrackColor = Pl.Accent,
                checkedThumbColor = Color.White,
                uncheckedTrackColor = Pl.Raised,
                uncheckedBorderColor = Pl.Line,
                uncheckedThumbColor = Pl.Muted,
            ),
        )
    }
}
