package dev.phonelink.app

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

/**
 * Mute and speaker for a call whose notification has no such buttons (Samsung's, for one):
 * Android's audio controls. They need MODIFY_AUDIO_SETTINGS (granted without asking). Not
 * every phone applies them to a carrier call made by its own dialer, so each change is read
 * back and reported honestly.
 */
object CallAudio {
    fun toggleMute(context: Context): String {
        val am = context.getSystemService(AudioManager::class.java) ?: error("No audio service")
        val want = !am.isMicrophoneMute
        am.isMicrophoneMute = want
        if (am.isMicrophoneMute != want) error("Your phone didn't let EchoTwin mute the call")
        return if (want) "Microphone muted" else "Microphone on"
    }

    @Suppress("DEPRECATION")
    fun toggleSpeaker(context: Context): String {
        val am = context.getSystemService(AudioManager::class.java) ?: error("No audio service")
        if (Build.VERSION.SDK_INT >= 31) {
            val onSpeaker = am.communicationDevice?.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
            if (onSpeaker) {
                am.clearCommunicationDevice()
                return "Speaker off"
            }
            val speaker = am.availableCommunicationDevices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                ?: error("This phone has no speaker to switch to")
            if (!am.setCommunicationDevice(speaker)) error("Your phone didn't let EchoTwin switch the speaker on")
            return "Speaker on"
        }
        val want = !am.isSpeakerphoneOn
        am.isSpeakerphoneOn = want
        if (am.isSpeakerphoneOn != want) error("Your phone didn't let EchoTwin switch the speaker")
        return if (want) "Speaker on" else "Speaker off"
    }
}
