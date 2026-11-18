package dev.phonelink.app

import android.app.ActivityOptions
import android.app.Notification
import android.app.Person
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.service.notification.StatusBarNotification
import android.telephony.TelephonyManager
import dev.phonelink.core.CallActionFfi
import dev.phonelink.core.CallPhaseFfi

/**
 * Calls on this phone, for the computer's call card (Answer / Decline), its in-call buttons
 * (End, Mute, Speaker, Hold: whatever the call notification offers) and its
 * pause-the-music-during-calls. Read from the dialer's call notification (any app that
 * posts a proper call notification, WhatsApp included). If no such notification shows up,
 * the phone's call state is the fallback (ringing, without a name).
 */
object CallWatcher {
    private data class Call(val phase: CallPhaseFfi, val caller: String, val actions: List<CallActionFfi> = emptyList())

    private val main = Handler(Looper.getMainLooper())
    /** Call notifications by key. Touched on the main thread only. */
    private val calls = LinkedHashMap<String, Call>()
    private var telephonyRinging = false
    private var last = Call(CallPhaseFfi.IDLE, "") to ""
    private val answerWords = Regex("(?i)answer|accept|pick up")

    /** Which button a call notification action is, by its title (any language: best effort). */
    private val kinds = listOf(
        CallActionFfi.ANSWER to answerWords,
        CallActionFfi.DECLINE to Regex("(?i)decline|reject|dismiss"),
        CallActionFfi.END to Regex("(?i)hang ?up|end|disconnect"),
        CallActionFfi.MUTE to Regex("(?i)mute|microphone|\\bmic\\b"),
        CallActionFfi.SPEAKER to Regex("(?i)speaker|loudspeaker|speakerphone"),
        CallActionFfi.HOLD to Regex("(?i)hold|resume"),
    )

    /** Android's own tag first (Mute / Unmute), else the title. */
    internal fun classify(title: String, semantic: Int): CallActionFfi? = when (semantic) {
        Notification.Action.SEMANTIC_ACTION_MUTE, Notification.Action.SEMANTIC_ACTION_UNMUTE -> CallActionFfi.MUTE
        else -> kinds.firstOrNull { it.second.containsMatchIn(title) }?.first
    }

    private fun kindOf(a: Notification.Action): CallActionFfi? =
        classify(a.title?.toString().orEmpty(), a.semanticAction)

    /** What the last call notification offered, for Settings → Calls (a screenshot tells all). */
    val lastSeen = kotlinx.coroutines.flow.MutableStateFlow("")

    @Suppress("DEPRECATION")
    private fun describe(n: Notification): String {
        val buttons = n.actions.orEmpty().joinToString(", ") { a ->
            val kind = kindOf(a)?.name?.lowercase() ?: "?"
            "\"${a.title}\" → $kind" + if (a.semanticAction != 0) " (tag ${a.semanticAction})" else ""
        }.ifEmpty { "no buttons" }
        val custom = n.contentView != null || n.bigContentView != null
        return buttons + if (custom) " · plus a custom layout EchoTwin can't read" else ""
    }

    fun isCall(sbn: StatusBarNotification): Boolean =
        sbn.notification.category == Notification.CATEGORY_CALL

    /** A call notification was posted or updated (from MirrorService). */
    fun posted(sbn: StatusBarNotification) {
        val n = sbn.notification
        val extras = n.extras
        val incoming = extras.getInt("android.callType", 0) == 1 ||
            n.actions?.any { answerWords.containsMatchIn(it.title ?: "") } == true
        val person = if (Build.VERSION.SDK_INT >= 33) {
            extras.getParcelable("android.callPerson", Person::class.java)
        } else {
            @Suppress("DEPRECATION")
            extras.getParcelable<Person>("android.callPerson")
        }
        val caller = person?.name?.toString()
            ?: extras.getCharSequence(Notification.EXTRA_TITLE)?.toString()
            ?: ""
        val phase = if (incoming) CallPhaseFfi.RINGING else CallPhaseFfi.ACTIVE
        var actions = n.actions.orEmpty().mapNotNull(::kindOf).distinct()
        if (phase == CallPhaseFfi.ACTIVE) {
            // Not offered as buttons the computer can press: done with Android's audio
            // controls instead (CallAudio), if the phone allows it.
            actions = actions + listOf(CallActionFfi.MUTE, CallActionFfi.SPEAKER).filter { it !in actions }
        }
        lastSeen.value = describe(n)
        val call = Call(phase, caller, actions)
        main.post {
            calls[sbn.key] = call
            update()
        }
    }

    fun removed(key: String) = main.post {
        if (calls.remove(key) != null) update()
    }

    /** Notification access went away: we can't see calls any more. */
    fun clear() = main.post {
        calls.clear()
        update()
    }

    /** The phone's own call state (needs the Phone permission). */
    fun telephony(state: Int) = main.post {
        when (state) {
            TelephonyManager.CALL_STATE_RINGING -> {
                telephonyRinging = true
                // Give the dialer's notification a moment; it carries the name.
                main.postDelayed({ if (telephonyRinging) update() }, 1500)
            }
            TelephonyManager.CALL_STATE_IDLE -> {
                telephonyRinging = false
                update()
            }
            else -> {
                telephonyRinging = false
                update()
            }
        }
    }

    /** Send the current state again (a computer just connected). */
    fun resend() = main.post {
        val (call, key) = last
        if (call.phase != CallPhaseFfi.IDLE) LinkManager.sendCallInfo(call.phase, call.caller, key, call.actions)
    }

    private fun update() {
        val ringing = calls.entries.firstOrNull { it.value.phase == CallPhaseFfi.RINGING }
        val any = ringing ?: calls.entries.firstOrNull()
        val now = when {
            any != null -> any.value to any.key
            telephonyRinging -> Call(CallPhaseFfi.RINGING, "") to ""
            else -> Call(CallPhaseFfi.IDLE, "") to ""
        }
        if (now != last) {
            last = now
            LinkManager.sendCallInfo(now.first.phase, now.first.caller, now.second, now.first.actions)
        }
    }

    /** "Decline" from an older computer: Decline, or End if that's all there is. */
    fun decline(service: MirrorService, key: String): Result<String> =
        act(service, key, CallActionFfi.DECLINE).recoverCatching { act(service, key, CallActionFfi.END).getOrThrow() }

    /** A button pressed on the computer: press the same button of the call notification. */
    fun act(service: MirrorService, key: String, kind: CallActionFfi): Result<String> = runCatching {
        val sbn = service.activeNotifications?.firstOrNull { it.key == key }
            ?: error("That call is no longer there")
        val action = sbn.notification.actions?.firstOrNull { kindOf(it) == kind }
            ?: return@runCatching when (kind) {
                CallActionFfi.MUTE -> CallAudio.toggleMute(service)
                CallActionFfi.SPEAKER -> CallAudio.toggleSpeaker(service)
                else -> error("Your phone's call screen doesn't offer that")
            }
        if (Build.VERSION.SDK_INT >= 34) {
            // Answer may open the call screen: allow that from the background.
            val opts = ActivityOptions.makeBasic()
                .setPendingIntentBackgroundActivityStartMode(ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED)
                .toBundle()
            action.actionIntent.send(service, 0, null, null, null, null, opts)
        } else {
            action.actionIntent.send()
        }
        when (kind) {
            CallActionFfi.ANSWER -> "Answered on your phone"
            CallActionFfi.DECLINE -> "Call declined"
            CallActionFfi.END -> "Call ended"
            else -> "Done"
        }
    }
}
