package dev.phonelink.app

import android.Manifest
import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.ContactsContract
import android.provider.Telephony
import android.telephony.SmsManager
import androidx.core.content.ContextCompat
import dev.phonelink.core.PhoneSms
import dev.phonelink.core.PhoneSmsThread
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * SMS for the computer's Messages tab: reads conversations from the phone's SMS store,
 * watches it for changes, and sends texts the computer asks for.
 *
 * Needs READ_SMS (and SEND_SMS to send, READ_CONTACTS for names), asked for only when you
 * tap "Allow SMS access". Only regular SMS: RCS "chat" messages are not readable by any
 * app other than the messaging app itself.
 */
object SmsSync {
    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private var observer: ContentObserver? = null
    private const val THREADS = 30
    private const val PER_THREAD = 30
    private const val SCAN_LIMIT = 4000
    private const val SEND_TIMEOUT_MS = 120_000L

    /** Last content sent per conversation, so a change elsewhere doesn't resend everything. */
    private val sentHash = ConcurrentHashMap<Long, Int>()
    private val names = ConcurrentHashMap<String, String>()
    private val requestCodes = AtomicInteger(5000)

    fun init(context: Context) {
        app = context.applicationContext
    }

    private fun granted(p: String) =
        ContextCompat.checkSelfPermission(app, p) == PackageManager.PERMISSION_GRANTED

    fun canRead() = granted(Manifest.permission.READ_SMS)
    fun canSend() = granted(Manifest.permission.SEND_SMS)

    val permissions = arrayOf(
        Manifest.permission.READ_SMS,
        Manifest.permission.SEND_SMS,
        Manifest.permission.READ_CONTACTS,
    )

    /** Watch the SMS store (call again after the permission is granted; idempotent). */
    fun start() = main.post {
        if (observer != null || !canRead()) return@post
        val o = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) = changed()
        }
        try {
            app.contentResolver.registerContentObserver(Telephony.Sms.CONTENT_URI, true, o)
            observer = o
        } catch (e: SecurityException) {
            // permission revoked meanwhile
        }
    }

    private val debounced = Runnable { LinkManager.smsChanged() }

    private fun changed() {
        main.removeCallbacks(debounced)
        main.postDelayed(debounced, 800)
    }

    // ------------------------------------------------------------------ reading

    private val COLUMNS = arrayOf(
        Telephony.Sms._ID,
        Telephony.Sms.THREAD_ID,
        Telephony.Sms.ADDRESS,
        Telephony.Sms.BODY,
        Telephony.Sms.DATE,
        Telephony.Sms.TYPE,
        Telephony.Sms.READ,
    )

    private class Row(
        val id: Long,
        val thread: Long,
        val address: String,
        val body: String,
        val date: Long,
        val type: Int,
        val read: Boolean,
    )

    private fun query(selection: String?, args: Array<String>?, max: Int, each: (Row) -> Boolean) {
        if (!canRead()) return
        try {
            app.contentResolver.query(Telephony.Sms.CONTENT_URI, COLUMNS, selection, args, "${Telephony.Sms.DATE} DESC")?.use { c ->
                var n = 0
                while (n < max && c.moveToNext()) {
                    n++
                    val type = c.getInt(5)
                    if (type == Telephony.Sms.MESSAGE_TYPE_DRAFT) continue
                    val row = Row(
                        id = c.getLong(0),
                        thread = c.getLong(1),
                        address = c.getString(2) ?: "",
                        body = c.getString(3) ?: "",
                        date = c.getLong(4),
                        type = type,
                        read = c.getInt(6) != 0,
                    )
                    if (!each(row)) break
                }
            }
        } catch (e: Exception) {
            // permission revoked, or the store is busy: the next change tries again
        }
    }

    private fun toSms(r: Row) = PhoneSms(
        id = r.id.toULong(),
        fromMe = r.type != Telephony.Sms.MESSAGE_TYPE_INBOX,
        body = r.body,
        dateMs = r.date.coerceAtLeast(0).toULong(),
        state = when (r.type) {
            Telephony.Sms.MESSAGE_TYPE_INBOX -> "received"
            Telephony.Sms.MESSAGE_TYPE_SENT -> "sent"
            Telephony.Sms.MESSAGE_TYPE_FAILED -> "failed"
            else -> "sending" // outbox, queued
        },
        read = r.read,
    )

    /** Contact name for a number (needs READ_CONTACTS; otherwise the number). */
    private fun nameFor(address: String): String {
        if (address.isBlank()) return "Unknown"
        names[address]?.let { return it }
        var name = address
        if (granted(Manifest.permission.READ_CONTACTS)) {
            try {
                val uri = Uri.withAppendedPath(ContactsContract.PhoneLookup.CONTENT_FILTER_URI, Uri.encode(address))
                app.contentResolver.query(uri, arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)?.use { c ->
                    if (c.moveToFirst()) c.getString(0)?.takeIf { it.isNotBlank() }?.let { name = it }
                }
            } catch (e: Exception) {
                // no contacts provider, or permission revoked
            }
        }
        names[address] = name
        return name
    }

    private fun build(id: Long, rows: List<Row>, hasOlder: Boolean): PhoneSmsThread {
        val address = rows.firstOrNull { it.address.isNotBlank() }?.address ?: ""
        return PhoneSmsThread(
            id = id.toULong(),
            address = address,
            name = nameFor(address),
            messages = rows.take(PER_THREAD).map(::toSms).reversed(),
            hasOlder = hasOlder || rows.size > PER_THREAD,
        )
    }

    /** The most recent conversations with their latest messages. */
    fun snapshot(): List<PhoneSmsThread> {
        val byThread = LinkedHashMap<Long, MutableList<Row>>()
        var scanned = 0
        query(null, null, SCAN_LIMIT) { r ->
            scanned++
            val list = byThread[r.thread]
            when {
                list != null -> if (list.size <= PER_THREAD) list.add(r)
                byThread.size < THREADS -> byThread[r.thread] = mutableListOf(r)
            }
            true
        }
        val cut = scanned >= SCAN_LIMIT
        val threads = byThread.map { (id, rows) -> build(id, rows, cut) }
        sentHash.clear()
        threads.forEach { sentHash[it.id.toLong()] = it.hashCode() }
        return threads
    }

    /** Conversations that changed since they were last sent (new, sent, deleted message…). */
    fun changedThreads(): List<PhoneSmsThread> {
        val recent = LinkedHashSet<Long>()
        query(null, null, 200) { r -> recent.add(r.thread); recent.size < 10 }
        return recent.mapNotNull { thread(it) }.filter { t ->
            val h = t.hashCode()
            (sentHash.put(t.id.toLong(), h) != h)
        }
    }

    fun thread(id: Long): PhoneSmsThread? {
        val rows = mutableListOf<Row>()
        query("${Telephony.Sms.THREAD_ID} = ?", arrayOf(id.toString()), PER_THREAD + 1) { rows.add(it); true }
        return if (rows.isEmpty()) null else build(id, rows, false)
    }

    /** Messages of `thread` before `beforeMs`, oldest first, and whether there are more. */
    fun older(thread: Long, beforeMs: Long): Pair<List<PhoneSms>, Boolean> {
        val rows = mutableListOf<Row>()
        query(
            "${Telephony.Sms.THREAD_ID} = ? AND ${Telephony.Sms.DATE} < ?",
            arrayOf(thread.toString(), beforeMs.toString()),
            PER_THREAD + 1,
        ) { rows.add(it); true }
        return rows.take(PER_THREAD).map(::toSms).reversed() to (rows.size > PER_THREAD)
    }

    // ------------------------------------------------------------------ sending

    /**
     * Send `body` to `address` from this phone's default SMS SIM. `done(sent, message)` is
     * called once, when the network accepted every part, one part failed, or after 2 min.
     * Android records the sent message in the SMS store itself.
     */
    fun send(address: String, body: String, done: (Boolean, String) -> Unit) {
        if (!canSend()) {
            done(false, "Allow SMS in the PhoneLink app on the phone")
            return
        }
        val sms: SmsManager = try {
            if (Build.VERSION.SDK_INT >= 31) app.getSystemService(SmsManager::class.java)
            else @Suppress("DEPRECATION") SmsManager.getDefault()
        } catch (e: Exception) {
            null
        } ?: run {
            done(false, "This phone can't send SMS")
            return
        }
        val parts = sms.divideMessage(body)
        val action = "dev.phonelink.app.SMS_SENT." + requestCodes.incrementAndGet()
        var left = parts.size
        var finished = false
        lateinit var receiver: BroadcastReceiver
        val finish = { ok: Boolean, msg: String ->
            if (!finished) {
                finished = true
                runCatching { app.unregisterReceiver(receiver) }
                done(ok, msg)
            }
        }
        receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (resultCode != Activity.RESULT_OK) {
                    finish(false, reason(resultCode))
                } else if (--left == 0) {
                    finish(true, "")
                }
            }
        }
        ContextCompat.registerReceiver(app, receiver, IntentFilter(action), ContextCompat.RECEIVER_NOT_EXPORTED)
        val intents = ArrayList(parts.map {
            PendingIntent.getBroadcast(
                app,
                requestCodes.incrementAndGet(),
                Intent(action).setPackage(app.packageName),
                PendingIntent.FLAG_IMMUTABLE,
            )
        })
        main.postDelayed({ finish(false, "The network didn't confirm it") }, SEND_TIMEOUT_MS)
        try {
            sms.sendMultipartTextMessage(address, null, parts, intents, null)
        } catch (e: Exception) {
            main.post { finish(false, e.message ?: "Couldn't send") }
        }
    }

    private fun reason(code: Int) = when (code) {
        SmsManager.RESULT_ERROR_NO_SERVICE -> "No mobile service"
        SmsManager.RESULT_ERROR_RADIO_OFF -> "Airplane mode is on"
        SmsManager.RESULT_ERROR_NULL_PDU -> "The message couldn't be encoded"
        else -> "The network refused it (error $code)"
    }
}
