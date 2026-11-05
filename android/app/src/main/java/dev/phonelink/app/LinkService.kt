package dev.phonelink.app

import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.wifi.WifiManager
import androidx.core.app.ServiceCompat
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import dev.phonelink.core.Device
import dev.phonelink.core.PhoneLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Keeps the link alive in the background. A foreground service is the only thing One UI
 * reliably leaves running; it also holds the multicast lock that mDNS discovery needs and
 * tells the core about network changes so it reconnects at once.
 */
class LinkService : LifecycleService() {
    private var multicastLock: WifiManager.MulticastLock? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun onCreate() {
        super.onCreate()
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
            runCatching { link.startDiscovery() }
            watchNetwork(link)
        }
        lifecycleScope.launch {
            LinkManager.devices.collect { Notifications.updateStatus(this@LinkService, statusText(it)) }
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
            connected.isNotEmpty() -> "Connected to ${connected.joinToString { it.name }}"
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
            override fun onAvailable(network: Network) = link.onNetworkChanged()
            override fun onLost(network: Network) = link.onNetworkChanged()
            override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) =
                link.onNetworkChanged()
        }
        cm.registerDefaultNetworkCallback(callback)
        networkCallback = callback
    }

    override fun onDestroy() {
        networkCallback?.let { cb ->
            runCatching { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(cb) }
        }
        multicastLock?.takeIf { it.isHeld }?.release()
        super.onDestroy()
    }
}
