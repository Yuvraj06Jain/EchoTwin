package dev.phonelink.app

import android.content.Context
import android.os.Build
import android.provider.Settings
import dev.phonelink.core.Device
import dev.phonelink.core.EventListener
import dev.phonelink.core.LinkEvent
import dev.phonelink.core.PhoneLink
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/**
 * Process-wide owner of the Rust core. The foreground service keeps the process (and so
 * this object) alive; the UI only observes the flows.
 */
object LinkManager {
    private lateinit var app: Context

    @Volatile
    private var link: PhoneLink? = null

    private val _devices = MutableStateFlow<List<Device>>(emptyList())
    val devices: StateFlow<List<Device>> = _devices

    private val _messages = MutableStateFlow<List<String>>(emptyList())
    val messages: StateFlow<List<String>> = _messages

    private val _myId = MutableStateFlow("")
    val myId: StateFlow<String> = _myId

    fun init(context: Context) {
        app = context.applicationContext
    }

    private fun dataDir(context: Context) = File(context.filesDir, "phonelink")

    /** Starts the core on first use. Blocking: call from Dispatchers.IO. */
    @Synchronized
    fun link(): PhoneLink {
        link?.let { return it }
        val dir = dataDir(app).apply { mkdirs() }
        val started = PhoneLink.start(dir.path, deviceName(), listener)
        link = started
        _myId.value = started.id()
        refresh()
        return started
    }

    fun refresh() {
        link?.let { _devices.value = it.devices() }
    }

    /** Cheap check usable from a BroadcastReceiver (no core start-up). */
    fun hasPairedDevice(context: Context): Boolean {
        val peers = File(dataDir(context), "peers.json")
        return peers.exists() && peers.readText().contains("\"fingerprint\"")
    }

    private val listener = object : EventListener {
        override fun onEvent(event: LinkEvent) {
            refresh()
            if (event is LinkEvent.Text) {
                _messages.value = (listOf("${event.name}: ${event.text}") + _messages.value).take(50)
                Notifications.message(app, event.name, event.text)
            }
        }
    }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
}
