package dev.phonelink.app

import android.app.Application

class PhoneLinkApp : Application() {
    override fun onCreate() {
        super.onCreate()
        LinkManager.init(this)
        Notifications.createChannels(this)
    }
}
