package dev.phonelink.app

import android.content.ComponentName
import android.content.Context
import android.database.ContentObserver
import android.graphics.Bitmap
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSession
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import dev.phonelink.core.MediaCommand
import dev.phonelink.core.PhoneMedia
import java.io.ByteArrayOutputStream

/**
 * What is playing on the phone (Spotify, YouTube Music, …), for the computer's Now Playing
 * card, and the buttons pressed there.
 *
 * Uses the notification access EchoTwin already has: Android lets a notification listener
 * see the active media sessions. No extra permission, no microphone, no audio is captured:
 * only the title/artist/position the player itself publishes.
 *
 * Everything runs on the main thread (media callbacks arrive there); network sends go
 * through [LinkManager]'s background thread.
 */
object MediaWatcher {
    private val main = Handler(Looper.getMainLooper())
    private var context: Context? = null
    private var sessions: MediaSessionManager? = null
    private val controllers = LinkedHashMap<MediaSession.Token, Pair<MediaController, MediaController.Callback>>()
    private var volumeObserver: ContentObserver? = null

    /** Last state sent, and the album art the computer already has. */
    private var lastSent: PhoneMedia? = null
    private var lastSentAt = 0L
    private var artSent: String? = null

    private const val ART_PX = 200
    private const val ART_MAX_BYTES = 60 * 1024
    private const val DEBOUNCE_MS = 300L

    private val sessionsChanged = MediaSessionManager.OnActiveSessionsChangedListener { list ->
        track(list.orEmpty())
    }

    /** Called when notification access is (re)connected. */
    fun start(ctx: Context) = main.post {
        if (sessions != null) return@post
        val app = ctx.applicationContext
        val msm = app.getSystemService(MediaSessionManager::class.java) ?: return@post
        val component = ComponentName(app, MirrorService::class.java)
        try {
            msm.addOnActiveSessionsChangedListener(sessionsChanged, component, main)
            context = app
            sessions = msm
            track(msm.getActiveSessions(component))
        } catch (e: SecurityException) {
            return@post // notification access not granted (any more)
        }
        val observer = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) = schedule() // media volume changed
        }
        app.contentResolver.registerContentObserver(Settings.System.CONTENT_URI, true, observer)
        volumeObserver = observer
    }

    /** Notification access went away. */
    fun stop() = main.post {
        sessions?.removeOnActiveSessionsChangedListener(sessionsChanged)
        sessions = null
        controllers.values.forEach { (c, cb) -> c.unregisterCallback(cb) }
        controllers.clear()
        volumeObserver?.let { context?.contentResolver?.unregisterContentObserver(it) }
        volumeObserver = null
        lastSent = null
        artSent = null
        LinkManager.sendNowPlaying(null)
    }

    /** A computer connected: it gets the current state (and art) from scratch. */
    fun resend() = main.post {
        lastSent = null
        artSent = null
        schedule()
    }

    private fun track(list: List<MediaController>) {
        val now = list.associateBy { it.sessionToken }
        val gone = controllers.keys - now.keys
        for (t in gone) controllers.remove(t)?.let { (c, cb) -> c.unregisterCallback(cb) }
        for ((token, c) in now) {
            if (token in controllers) continue
            val cb = object : MediaController.Callback() {
                override fun onPlaybackStateChanged(state: PlaybackState?) = schedule()
                override fun onMetadataChanged(metadata: MediaMetadata?) = schedule()
                override fun onAudioInfoChanged(info: MediaController.PlaybackInfo) = schedule()
                override fun onSessionDestroyed() {
                    controllers.remove(token)?.let { (ctrl, me) -> ctrl.unregisterCallback(me) }
                    schedule()
                }
            }
            c.registerCallback(cb, main)
            controllers[token] = c to cb
        }
        schedule()
    }

    private val update = Runnable { push() }

    /** Coalesce bursts (a track change fires several callbacks): at most ~3 sends/second. */
    private fun schedule() {
        main.removeCallbacks(update)
        main.postDelayed(update, DEBOUNCE_MS)
    }

    private fun isActive(c: MediaController): Boolean {
        val s = c.playbackState?.state ?: return false
        return s != PlaybackState.STATE_NONE && s != PlaybackState.STATE_STOPPED &&
            s != PlaybackState.STATE_ERROR && c.metadata != null
    }

    /** The player to show: one that is playing, else the most recent paused one. */
    private fun current(): MediaController? {
        val all = controllers.values.map { it.first }
        return all.firstOrNull { it.playbackState?.state == PlaybackState.STATE_PLAYING && it.metadata != null }
            ?: all.firstOrNull(::isActive)
    }

    private fun position(state: PlaybackState): Long {
        var pos = state.position
        if (state.state == PlaybackState.STATE_PLAYING && state.lastPositionUpdateTime > 0) {
            val elapsed = SystemClock.elapsedRealtime() - state.lastPositionUpdateTime
            pos += (elapsed * state.playbackSpeed).toLong()
        }
        return pos.coerceAtLeast(0)
    }

    private fun appName(pkg: String): String {
        val pm = context?.packageManager ?: return pkg
        return try {
            pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
        } catch (e: Exception) {
            pkg
        }
    }

    /** (volume, max) of what the player plays through: the phone's media volume, or a cast device. */
    private fun volume(c: MediaController): Pair<Int, Int> {
        val info = c.playbackInfo
        if (info != null && info.playbackType == MediaController.PlaybackInfo.PLAYBACK_TYPE_REMOTE) {
            return info.currentVolume to info.maxVolume
        }
        val am = context?.getSystemService(AudioManager::class.java) ?: return 0 to 0
        return am.getStreamVolume(AudioManager.STREAM_MUSIC) to am.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
    }

    private fun push() {
        val c = current()
        if (c == null) {
            if (lastSent != null || lastSentAt == 0L) {
                lastSent = null
                lastSentAt = SystemClock.elapsedRealtime()
                LinkManager.sendNowPlaying(null)
            }
            return
        }
        val meta = c.metadata
        val state = c.playbackState ?: return
        val actions = state.actions
        val (vol, max) = volume(c)
        val title = meta?.getString(MediaMetadata.METADATA_KEY_TITLE)
            ?: meta?.getString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE) ?: ""
        val artist = meta?.getString(MediaMetadata.METADATA_KEY_ARTIST)
            ?: meta?.getString(MediaMetadata.METADATA_KEY_ALBUM_ARTIST)
            ?: meta?.getString(MediaMetadata.METADATA_KEY_DISPLAY_SUBTITLE) ?: ""
        val album = meta?.getString(MediaMetadata.METADATA_KEY_ALBUM) ?: ""
        val art = meta?.let {
            it.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART)
                ?: it.getBitmap(MediaMetadata.METADATA_KEY_ART)
                ?: it.getBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON)
        }
        val artKey = if (art == null) "" else "${c.packageName}|$title|$artist|$album|${art.width}x${art.height}".hashCode().toUInt().toString(16)
        val media = PhoneMedia(
            app = appName(c.packageName),
            `package` = c.packageName,
            title = title,
            artist = artist,
            album = album,
            durationMs = (meta?.getLong(MediaMetadata.METADATA_KEY_DURATION) ?: 0L).coerceAtLeast(0).toULong(),
            positionMs = position(state).toULong(),
            playing = state.state == PlaybackState.STATE_PLAYING,
            canPrevious = (actions and PlaybackState.ACTION_SKIP_TO_PREVIOUS) != 0L,
            canNext = (actions and PlaybackState.ACTION_SKIP_TO_NEXT) != 0L,
            volume = vol.coerceAtLeast(0).toUInt(),
            maxVolume = max.coerceAtLeast(0).toUInt(),
            artKey = artKey,
        )
        if (!changed(media)) return
        if (art != null && artKey != artSent) {
            encodeArt(art)?.let { LinkManager.sendMediaArt(artKey, it) }
            artSent = artKey
        }
        lastSent = media
        lastSentAt = SystemClock.elapsedRealtime()
        LinkManager.sendNowPlaying(media)
    }

    /**
     * Worth sending? Anything but the position changed, or the position jumped (a seek):
     * while playing, the computer moves the position on by itself.
     */
    private fun changed(m: PhoneMedia): Boolean {
        val last = lastSent ?: return true
        if (m.copy(positionMs = 0u) != last.copy(positionMs = 0u)) return true
        val expected = if (last.playing) {
            last.positionMs.toLong() + (SystemClock.elapsedRealtime() - lastSentAt)
        } else {
            last.positionMs.toLong()
        }
        return kotlin.math.abs(m.positionMs.toLong() - expected) > 2_500
    }

    /** Small JPEG for the computer's card (≤ 60 KB). */
    private fun encodeArt(src: Bitmap): ByteArray? = try {
        val scale = ART_PX.toFloat() / maxOf(src.width, src.height)
        val bmp = if (scale < 1f) {
            Bitmap.createScaledBitmap(src, (src.width * scale).toInt().coerceAtLeast(1), (src.height * scale).toInt().coerceAtLeast(1), true)
        } else {
            src
        }
        var quality = 85
        var bytes: ByteArray
        do {
            val out = ByteArrayOutputStream()
            bmp.compress(Bitmap.CompressFormat.JPEG, quality, out)
            bytes = out.toByteArray()
            quality -= 15
        } while (bytes.size > ART_MAX_BYTES && quality > 20)
        bytes.takeIf { it.size <= ART_MAX_BYTES }
    } catch (e: Exception) {
        null
    }

    /** A button pressed on the computer. Runs on the main thread; answers on the caller's. */
    fun control(command: MediaCommand) = main.post {
        val c = current()
        when (command) {
            is MediaCommand.PlayPause -> c?.transportControls?.let { tc ->
                if (c.playbackState?.state == PlaybackState.STATE_PLAYING) tc.pause() else tc.play()
            }
            is MediaCommand.Next -> c?.transportControls?.skipToNext()
            is MediaCommand.Previous -> c?.transportControls?.skipToPrevious()
            is MediaCommand.VolumeUp -> adjustVolume(c, AudioManager.ADJUST_RAISE)
            is MediaCommand.VolumeDown -> adjustVolume(c, AudioManager.ADJUST_LOWER)
            is MediaCommand.SetVolume -> setVolume(c, command.level.toInt())
        }
        schedule()
    }

    private fun adjustVolume(c: MediaController?, direction: Int) {
        val info = c?.playbackInfo
        if (info != null && info.playbackType == MediaController.PlaybackInfo.PLAYBACK_TYPE_REMOTE) {
            c.adjustVolume(direction, 0)
            return
        }
        context?.getSystemService(AudioManager::class.java)
            ?.adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, 0)
    }

    private fun setVolume(c: MediaController?, level: Int) {
        val info = c?.playbackInfo
        if (info != null && info.playbackType == MediaController.PlaybackInfo.PLAYBACK_TYPE_REMOTE) {
            c.setVolumeTo(level.coerceIn(0, info.maxVolume), 0)
            return
        }
        val am = context?.getSystemService(AudioManager::class.java) ?: return
        val max = am.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
        // No volume panel pops up on the phone (flags = 0).
        runCatching { am.setStreamVolume(AudioManager.STREAM_MUSIC, level.coerceIn(0, max), 0) }
    }
}
