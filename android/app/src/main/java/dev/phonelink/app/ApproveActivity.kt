package dev.phonelink.app

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.fragment.app.FragmentActivity
import dev.phonelink.app.ui.PhoneLinkTheme
import dev.phonelink.app.ui.Pl

/**
 * "MARK-I asks for admin access (sudo) · code A3F9": Approve (fingerprint) or Deny.
 * Closes itself (as Deny) after 30 seconds. Only approve if the computer shows the same code.
 */
class ApproveActivity : FragmentActivity() {
    private val main = Handler(Looper.getMainLooper())
    private var answered = false
    private var status by mutableStateOf<String?>(null)

    private val device get() = intent.getStringExtra(EXTRA_DEVICE).orEmpty()
    private val requestId get() = intent.getStringExtra(EXTRA_REQUEST).orEmpty()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val name = intent.getStringExtra(EXTRA_NAME).orEmpty()
        val what = intent.getStringExtra(EXTRA_WHAT).orEmpty()
        val host = intent.getStringExtra(EXTRA_HOST).orEmpty()
        val nonce = intent.getStringExtra(EXTRA_NONCE).orEmpty()
        val code = intent.getStringExtra(EXTRA_CODE).orEmpty()
        if (device.isEmpty() || requestId.isEmpty() || nonce.isEmpty()) {
            finish()
            return
        }
        main.postDelayed({ answer(false) }, 30_000)
        setContent {
            PhoneLinkTheme {
                Column(
                    Modifier.fillMaxSize().background(Pl.Bg).padding(24.dp),
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text("$name asks for admin access", fontSize = 20.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(4.dp))
                    Text("$what on $host", color = Pl.Muted)
                    Spacer(Modifier.height(24.dp))
                    Box(Modifier.background(Pl.AccentSoft, RoundedCornerShape(18.dp)).padding(horizontal = 28.dp, vertical = 14.dp)) {
                        Text(code, fontSize = 40.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold, color = Pl.AccentText)
                    }
                    Spacer(Modifier.height(16.dp))
                    Text(
                        "Approve only if your computer shows this code right now.",
                        color = Pl.Warn, textAlign = TextAlign.Center,
                    )
                    status?.let {
                        Spacer(Modifier.height(10.dp))
                        Text(it, color = Pl.Muted, textAlign = TextAlign.Center)
                    }
                    Spacer(Modifier.height(28.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        OutlinedButton(onClick = { answer(false) }, modifier = Modifier.weight(1f)) { Text("Deny", color = Pl.Text) }
                        Button(
                            onClick = { approve(name, what, host, nonce) },
                            modifier = Modifier.weight(1f),
                            colors = ButtonDefaults.buttonColors(containerColor = Pl.Accent, contentColor = Color.White),
                        ) { Text("Approve") }
                    }
                }
            }
        }
    }

    private fun approve(name: String, what: String, host: String, nonce: String) {
        if (answered) return
        val message = dev.phonelink.core.approveMessage(what, host, nonce)
        Approver.sign(this, name, message) { result ->
            result.fold(
                onSuccess = { sig -> respond(true, sig) },
                onFailure = { status = it.message ?: "Not approved" },
            )
        }
    }

    private fun answer(approved: Boolean) = respond(approved, ByteArray(0))

    private fun respond(approved: Boolean, signature: ByteArray) {
        if (answered) return
        answered = true
        main.removeCallbacksAndMessages(null)
        LinkManager.approveResponse(device, requestId, approved, signature)
        Notifications.cancelApprove(this)
        finish()
    }

    override fun onDestroy() {
        // Closed some other way (back, swiped away): that's a Deny.
        if (!answered && isFinishing) answer(false)
        super.onDestroy()
    }

    companion object {
        const val EXTRA_DEVICE = "device"
        const val EXTRA_NAME = "name"
        const val EXTRA_REQUEST = "request"
        const val EXTRA_WHAT = "what"
        const val EXTRA_HOST = "host"
        const val EXTRA_NONCE = "nonce"
        const val EXTRA_CODE = "code"
    }
}
