package dev.phonelink.app

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.view.MotionEvent
import android.view.View
import dev.phonelink.core.Finger

/**
 * The Touchpad tab's touch surface. It does not interpret gestures: it forwards every finger
 * as it is (slot, id, position 0..1) and the laptop's own touchpad stack decides what a tap,
 * drag, scroll or 3-finger swipe means, exactly as on the real touchpad. (Compose's
 * pointerInput, as in the presentation pad, follows one finger; this follows all five.)
 *
 * Ported from TouchTwin (`TouchPadView.kt`); the dots are EchoTwin green, and the surface
 * itself is drawn by the Compose screen around it.
 */
@SuppressLint("ViewConstructor")
class TouchPadView(
    context: Context,
    /** Every change in the fingers that are down (empty list = all lifted). */
    private val onFingers: (List<Finger>) -> Unit,
    /** The pad's physical size in millimetres changed (layout, rotation). */
    private val onSize: (widthMm: Float, heightMm: Float) -> Unit,
) : View(context) {

    private val maxSlots = 5

    /** Android pointer id -> our slot (lowest free slot, stable while the finger is down). */
    private val slotOf = HashMap<Int, Int>()
    /** Our touch id per slot: new for every touch, so a re-touch is a new touch. */
    private val touchId = IntArray(maxSlots)
    private var nextTouchId = 1

    private val dots = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0x553DBB82 }
    private val dotRadius = 22f * resources.displayMetrics.density

    init {
        isHapticFeedbackEnabled = false
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        val dm = resources.displayMetrics
        if (w > 0 && h > 0) onSize(w / dm.xdpi * 25.4f, h / dm.ydpi * 25.4f)
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        super.onLayout(changed, left, top, right, bottom)
        // Keep the system back gesture off the pad's edges (Android allows 200 dp per edge).
        val maxH = (200 * resources.displayMetrics.density).toInt()
        val edge = (24 * resources.displayMetrics.density).toInt()
        val h = bottom - top
        val y0 = ((h - maxH) / 2).coerceAtLeast(0)
        systemGestureExclusionRects = listOf(
            Rect(0, y0, edge, y0 + maxH.coerceAtMost(h)),
            Rect(width - edge, y0, width, y0 + maxH.coerceAtMost(h)),
        )
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        val lifting = when (e.actionMasked) {
            MotionEvent.ACTION_POINTER_UP -> e.getPointerId(e.actionIndex)
            MotionEvent.ACTION_UP -> e.getPointerId(0)
            else -> null
        }
        if (e.actionMasked == MotionEvent.ACTION_CANCEL) {
            slotOf.clear()
            send(emptyList())
            return true
        }
        val fingers = ArrayList<Finger>(e.pointerCount)
        val w = width.coerceAtLeast(1).toFloat()
        val h = height.coerceAtLeast(1).toFloat()
        for (i in 0 until e.pointerCount) {
            val pid = e.getPointerId(i)
            if (pid == lifting) continue
            val slot = slotOf[pid] ?: freeSlot()?.also { s ->
                slotOf[pid] = s
                touchId[s] = nextTouchId
                nextTouchId = if (nextTouchId >= 65535) 1 else nextTouchId + 1
            } ?: continue // a sixth finger: ignored, like a real touchpad
            fingers += Finger(
                slot = slot.toUByte(),
                id = touchId[slot].toUShort(),
                x = e.getX(i) / w,
                y = e.getY(i) / h,
            )
        }
        if (lifting != null) slotOf.remove(lifting)
        send(fingers)
        return true
    }

    /** Lift everything (the tab is left while fingers are down). */
    fun liftAll() {
        if (slotOf.isEmpty() && shown.isEmpty()) return
        slotOf.clear()
        send(emptyList())
    }

    private fun freeSlot(): Int? = (0 until maxSlots).firstOrNull { it !in slotOf.values }

    private var shown: List<Finger> = emptyList()

    private fun send(fingers: List<Finger>) {
        onFingers(fingers)
        shown = fingers
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        // Soft dots under the fingers: shows the pad is alive, nothing more.
        for (f in shown) canvas.drawCircle(f.x * width, f.y * height, dotRadius, dots)
    }
}
