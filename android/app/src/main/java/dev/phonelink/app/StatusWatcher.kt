package dev.phonelink.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.telephony.SignalStrength
import android.telephony.TelephonyCallback
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import androidx.core.content.ContextCompat

/**
 * This phone's battery and network ("5G", "4G", "Wi-Fi"…) for the computer's status display.
 * Sent only to connected computers, when something changes (at most once a second).
 */
object StatusWatcher {
    const val PHONE_PERMISSION = Manifest.permission.READ_PHONE_STATE

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private var started = false

    @Volatile private var battery: Int? = null
    @Volatile private var charging = false
    @Volatile private var onWifi = false
    @Volatile private var onMobile = false
    @Volatile private var displayType = TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NONE
    @Volatile private var signal = 0
    private var lastSent: List<Any?>? = null
    private var telephonyCallback: Any? = null

    fun allowed(context: Context): Boolean =
        ContextCompat.checkSelfPermission(context, PHONE_PERMISSION) == PackageManager.PERMISSION_GRANTED

    private val batteryReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
            val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
            battery = if (level >= 0 && scale > 0) (level * 100 / scale).coerceIn(0, 100) else null
            val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
            charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
            changed()
        }
    }

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            onWifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
            onMobile = caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
            changed()
        }

        override fun onLost(network: Network) {
            onWifi = false
            onMobile = false
            changed()
        }
    }

    /** Called by LinkService; safe to call again (e.g. after the Phone permission is granted). */
    @Synchronized
    fun start(context: Context) {
        app = context.applicationContext
        if (!started) {
            started = true
            ContextCompat.registerReceiver(
                app, batteryReceiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED), ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            runCatching {
                app.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(networkCallback)
            }
        }
        listenToMobileNetwork()
    }

    /** 5G/4G label and signal bars (Android 12+; needs the Phone permission for the type). */
    @SuppressLint("MissingPermission")
    private fun listenToMobileNetwork() {
        if (telephonyCallback != null || Build.VERSION.SDK_INT < 31 || !allowed(app)) return
        val tm = app.getSystemService(TelephonyManager::class.java) ?: return
        val cb = object : TelephonyCallback(), TelephonyCallback.DisplayInfoListener,
            TelephonyCallback.SignalStrengthsListener {
            override fun onDisplayInfoChanged(info: TelephonyDisplayInfo) {
                displayType = info.overrideNetworkType
                changed()
            }

            override fun onSignalStrengthsChanged(strength: SignalStrength) {
                signal = strength.level.coerceIn(0, 4)
                changed()
            }
        }
        runCatching {
            tm.registerTelephonyCallback(app.mainExecutor, cb)
            telephonyCallback = cb
        }
    }

    @SuppressLint("MissingPermission")
    private fun mobileLabel(): String {
        if (displayType == TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA ||
            displayType == TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_ADVANCED
        ) return "5G"
        if (!allowed(app)) return "Mobile data"
        val type = runCatching { app.getSystemService(TelephonyManager::class.java)?.dataNetworkType }.getOrNull()
        return when (type) {
            TelephonyManager.NETWORK_TYPE_NR -> "5G"
            TelephonyManager.NETWORK_TYPE_LTE, TelephonyManager.NETWORK_TYPE_IWLAN -> "4G"
            TelephonyManager.NETWORK_TYPE_HSPAP, TelephonyManager.NETWORK_TYPE_HSPA,
            TelephonyManager.NETWORK_TYPE_HSDPA, TelephonyManager.NETWORK_TYPE_HSUPA,
            TelephonyManager.NETWORK_TYPE_UMTS, TelephonyManager.NETWORK_TYPE_EVDO_0,
            TelephonyManager.NETWORK_TYPE_EVDO_A, TelephonyManager.NETWORK_TYPE_EVDO_B,
            TelephonyManager.NETWORK_TYPE_TD_SCDMA -> "3G"
            TelephonyManager.NETWORK_TYPE_GSM, TelephonyManager.NETWORK_TYPE_EDGE,
            TelephonyManager.NETWORK_TYPE_GPRS, TelephonyManager.NETWORK_TYPE_CDMA,
            TelephonyManager.NETWORK_TYPE_1xRTT -> "2G"
            else -> "Mobile data"
        }
    }

    private fun current(): List<Any?> {
        val network = when {
            onWifi -> "Wi-Fi"
            onMobile -> mobileLabel()
            else -> ""
        }
        return listOf(battery, charging, network, if (onMobile) signal else 0)
    }

    private val sendNow = Runnable { send(force = false) }

    private fun changed() {
        main.removeCallbacks(sendNow)
        main.postDelayed(sendNow, 1000)
    }

    /** Send to connected computers; `force` after one (re)connects. */
    @Synchronized
    fun send(force: Boolean) {
        if (!::app.isInitialized) return
        val now = current()
        if (!force && now == lastSent) return
        lastSent = now
        LinkManager.sendPhoneStatus(now[0] as Int?, now[1] as Boolean, now[2] as String, now[3] as Int)
    }
}
