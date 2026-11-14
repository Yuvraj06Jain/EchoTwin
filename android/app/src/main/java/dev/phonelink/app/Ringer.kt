package dev.phonelink.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * "Ring phone" from the computer: the alarm sound at full alarm volume (so it is heard
 * even on silent), with vibration and a Stop button. Stops by itself after a minute; the
 * alarm volume is put back afterwards.
 */
object Ringer {
    private const val CHANNEL = "ring"
    private const val NOTIFICATION_ID = 7
    private const val MAX_MS = 60_000L

    private val main = Handler(Looper.getMainLooper())
    private var player: MediaPlayer? = null
    private var oldVolume: Int? = null
    private val autoStop = Runnable { stopNow() }
    private lateinit var app: Context

    fun start(context: Context, from: String) = main.post {
        app = context.applicationContext
        if (player != null) return@post
        val audio = app.getSystemService(AudioManager::class.java)
        oldVolume = audio.getStreamVolume(AudioManager.STREAM_ALARM)
        audio.setStreamVolume(AudioManager.STREAM_ALARM, audio.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0)
        val uri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
        player = try {
            MediaPlayer().apply {
                setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ALARM)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build()
                )
                setDataSource(app, uri)
                isLooping = true
                prepare()
                start()
            }
        } catch (e: Exception) {
            null // still vibrates and shows the notification
        }
        @Suppress("DEPRECATION")
        app.getSystemService(Vibrator::class.java)
            ?.vibrate(VibrationEffect.createWaveform(longArrayOf(0, 700, 500), 0))
        notify(from)
        main.postDelayed(autoStop, MAX_MS)
    }

    fun stop() = main.post { stopNow() }

    private fun stopNow() {
        if (!::app.isInitialized) return
        main.removeCallbacks(autoStop)
        player?.let {
            runCatching { it.stop() }
            it.release()
        }
        player = null
        app.getSystemService(Vibrator::class.java)?.cancel()
        oldVolume?.let {
            app.getSystemService(AudioManager::class.java).setStreamVolume(AudioManager.STREAM_ALARM, it, 0)
        }
        oldVolume = null
        NotificationManagerCompat.from(app).cancel(NOTIFICATION_ID)
    }

    private fun notify(from: String) {
        val nm = app.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Ringing from your computer", NotificationManager.IMPORTANCE_HIGH).apply {
                setSound(null, null) // the alarm sound plays separately
            }
        )
        val stop = PendingIntent.getBroadcast(
            app, 0, Intent(app, StopRingReceiver::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_link)
            .setContentTitle("$from is ringing this phone")
            .setContentText("Tap Stop to silence it")
            .setCategory(NotificationCompat.CATEGORY_ALARM)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setOngoing(true)
            .setContentIntent(stop)
            .addAction(0, "Stop", stop)
            .setDeleteIntent(stop)
            .build()
        runCatching { NotificationManagerCompat.from(app).notify(NOTIFICATION_ID, n) }
    }
}

/** The Stop button (not reachable by other apps). */
class StopRingReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        Ringer.stop()
    }
}
