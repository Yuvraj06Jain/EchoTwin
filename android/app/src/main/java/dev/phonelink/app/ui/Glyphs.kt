package dev.phonelink.app.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.unit.dp

/**
 * Icons the core Material set lacks, drawn from Material Icons path data (Apache License
 * 2.0), filled style, 24×24. Tinted like every other tile icon.
 */
object Glyphs {
    private fun icon(name: String, path: String): ImageVector =
        ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f)
            // Even-odd: the open lock's body is a frame with the keyhole dot inside.
            .addPath(pathData = addPathNodes(path), pathFillType = PathFillType.EvenOdd, fill = SolidColor(Color.Black))
            .build()

    /** An open padlock: "Unlock". */
    val LockOpen: ImageVector = icon(
        "LockOpen",
        "M12,17c1.1,0 2,-0.9 2,-2s-0.9,-2 -2,-2 -2,0.9 -2,2 0.9,2 2,2zM18,8h-1L17,6c0,-2.76 -2.24,-5 -5,-5S7,3.24 7,6h1.9" +
            "c0,-1.71 1.39,-3.1 3.1,-3.1 1.71,0 3.1,1.39 3.1,3.1v2L6,8c-1.1,0 -2,0.9 -2,2v10c0,1.1 0.9,2 2,2h12c1.1,0 2,-0.9 2,-2" +
            "L20,10c0,-1.1 -0.9,-2 -2,-2zM18,20L6,20L6,10h12v10z",
    )

    /** A speaker with a slash: "Mute". */
    val VolumeOff: ImageVector = icon(
        "VolumeOff",
        "M16.5,12c0,-1.77 -1.02,-3.29 -2.5,-4.03v2.21l2.45,2.45c0.03,-0.2 0.05,-0.41 0.05,-0.63zM19,12c0,0.94 -0.2,1.82 -0.54,2.64" +
            "l1.51,1.51C20.63,14.91 21,13.5 21,12c0,-4.28 -2.99,-7.86 -7,-8.77v2.06c2.89,0.86 5,3.54 5,6.71zM4.27,3L3,4.27 7.73,9" +
            "L3,9v6h4l5,5v-6.73l4.25,4.25c-0.67,0.52 -1.42,0.93 -2.25,1.18v2.06c1.38,-0.31 2.63,-0.95 3.69,-1.81L19.73,21 21,19.73" +
            "l-9,-9L4.27,3zM12,4L9.91,6.09 12,8.18L12,4z",
    )
}
