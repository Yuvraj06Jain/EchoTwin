package dev.phonelink.app

import android.app.Notification
import android.app.Person
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.service.notification.StatusBarNotification
import android.telephony.TelephonyManager
import dev.phonelink.core.CallPhaseFfi

/**
 * Calls on this phone, for the computer's "Mom is calling · Decline" popup and its
 * pause-the-music-during-calls. Read from the dialer's call notification (any app that
 * posts a proper call notification, WhatsApp included). If no such notification shows up,
 * the phone's call state is the fallback (ringing, without a name).
 */
object CallWatcher {
    private data class Call(val phase: CallPhaseFfi, val caller: String)

    private val main = Handler(Looper.getMainLooper())
    /** Call notifications by key. Touched on the main thread only. */
    private val calls = LinkedHashMap<String, Call>()
    private var telephonyRinging = false
    private var last: Triple<CallPhaseFfi, String, String> = Triple(CallPhaseFfi.IDLE, "", "")
    private val answerWords = Regex("(?i)answer|accept|pick up")
    private val declineWords = Regex("(?i)decline|reject|dismiss|hang ?up|end")

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
        val call = Call(if (incoming) CallPhaseFfi.RINGING else CallPhaseFfi.ACTIVE, caller)
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
        if (last.first != CallPhaseFfi.IDLE) LinkManager.sendCallState(last.first, last.second, last.third)
    }

    private fun update() {
        val ringing = calls.entries.firstOrNull { it.value.phase == CallPhaseFfi.RINGING }
        val any = ringing ?: calls.entries.firstOrNull()
        val now = when {
            any != null -> Triple(any.value.phase, any.value.caller, any.key)
            telephonyRinging -> Triple(CallPhaseFfi.RINGING, "", "")
            else -> Triple(CallPhaseFfi.IDLE, "", "")
        }
        if (now != last) {
            last = now
            LinkManager.sendCallState(now.first, now.second, now.third)
        }
    }

    /** "Decline" pressed on the computer: press the call notification's decline button. */
    fun decline(service: MirrorService, key: String): Result<String> = runCatching {
        val sbn = service.activeNotifications?.firstOrNull { it.key == key }
            ?: error("That call is no longer ringing")
        val action = sbn.notification.actions?.firstOrNull { declineWords.containsMatchIn(it.title ?: "") }
            ?: error("Can't decline this call from the computer")
        action.actionIntent.send()
        "Call declined"
    }
}
