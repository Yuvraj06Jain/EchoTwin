package dev.phonelink.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.wifi.WifiManager
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import dev.phonelink.core.Device
import dev.phonelink.core.PhoneLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * Keeps the link alive in the background. A foreground service is the only thing One UI
 * reliably leaves running; it also holds the multicast lock that mDNS discovery needs and
 * tells the core about network changes so it reconnects at once.
 */
class LinkService : LifecycleService() {
    private var multicastLock: WifiManager.MulticastLock? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    /** Unlocking the phone is a good moment to reconnect at once (no permission needed). */
    private val unlockReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_USER_PRESENT) LinkManager.nudge()
        }
    }

    override fun onCreate() {
        super.onCreate()
        ContextCompat.registerReceiver(
            this,
            unlockReceiver,
            IntentFilter(Intent.ACTION_USER_PRESENT),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        ServiceCompat.startForeground(
            this,
            Notifications.STATUS_ID,
            Notifications.status(this, "Starting…"),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        lifecycleScope.launch(Dispatchers.IO) {
            val link = try {
                LinkManager.link()
            } catch (e: Exception) {
                Notifications.updateStatus(this@LinkService, "Could not start: ${e.message}")
                return@launch
            }
            acquireMulticastLock()
            SmsSync.start()
            StatusWatcher.start(this@LinkService)
            BluetoothBackup.start()
            runCatching { link.startDiscovery() }
            watchNetwork(link)
        }
        lifecycleScope.launch {
            LinkManager.devices.combine(LinkManager.settingsVersion) { d, _ -> d }.collect { devices ->
                val actions = Notifications.Actions(
                    lock = devices.any { it.canLock },
                    clipboard = devices.any { it.canClipboard } && Prefs.clipboardSharing(this@LinkService),
                )
                Notifications.updateStatus(this@LinkService, statusText(devices), actions)
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)
        return START_STICKY // restarted by Android if it is ever killed
    }

    private fun statusText(devices: List<Device>): String {
        val connected = devices.filter { it.connected }
        return when {
            devices.isEmpty() -> "Not paired yet. Open the app to pair."
            connected.isNotEmpty() -> "Connected to ${connected.joinToString { it.name }}" +
                if (connected.any { it.via == "bluetooth" }) " over Bluetooth" else ""
            else -> "Waiting for ${devices.joinToString { it.name }}…"
        }
    }

    private fun acquireMulticastLock() {
        val wifi = applicationContext.getSystemService(WifiManager::class.java) ?: return
        multicastLock = wifi.createMulticastLock("phonelink-mdns").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun watchNetwork(link: PhoneLink) {
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                link.onNetworkChanged()
                BluetoothBackup.onNetworkChanged()
            }
            override fun onLost(network: Network) {
                link.onNetworkChanged()
                BluetoothBackup.onNetworkChanged()
            }
            override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) =
                link.onNetworkChanged()
        }
        cm.registerDefaultNetworkCallback(callback)
        networkCallback = callback
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(unlockReceiver) }
        networkCallback?.let { cb ->
            runCatching { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(cb) }
        }
        multicastLock?.takeIf { it.isHeld }?.release()
        super.onDestroy()
    }
}
