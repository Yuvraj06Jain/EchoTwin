import dev.phonelink.core.Finger
import dev.phonelink.core.PadEvent
import dev.phonelink.core.PadException
import dev.phonelink.core.PadListener
import dev.phonelink.core.TouchpadLink
import java.io.File
import java.net.DatagramSocket
import java.nio.file.Files
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.test.fail

/** The Touchpad tab's Kotlin API against the real `phonelink-touchpad run`. */
class TouchpadTest {
    private val bin = System.getenv("PHONELINK_BIN_DIR") ?: error("PHONELINK_BIN_DIR not set")

    private fun proc(env: Map<String, String>, vararg args: String): ProcessBuilder =
        ProcessBuilder(listOf("$bin/${args[0]}") + args.drop(1)).apply { environment().putAll(env) }

    private inline fun <reified T : PadEvent> LinkedBlockingQueue<PadEvent>.await(): T {
        val deadline = System.currentTimeMillis() + 15_000
        while (true) {
            val left = deadline - System.currentTimeMillis()
            val e = poll(maxOf(left, 0), TimeUnit.MILLISECONDS) ?: fail("timed out waiting for ${T::class.simpleName}")
            if (e is T) return e
        }
    }

    @Test
    fun kotlinTouchpadPairsWithTheRealServiceAndOpensThePad() {
        val tmp = Files.createTempDirectory("touchpad").toFile()
        val port = DatagramSocket(0).use { it.localPort }
        val env = mapOf(
            "PHONELINK_TOUCHPAD_SOCKET" to "$tmp/run/control.sock",
            "PHONELINK_TOUCHPAD_DATA_DIR" to "$tmp/state",
            "PHONELINK_TOUCHPAD_PORT" to "$port",
            "PHONELINK_TOUCHPAD_DEV_NO_AUTH" to "1", // no polkit dialog in a test
            "RUST_LOG" to "warn",
        )
        val service = proc(env, "phonelink-touchpad", "run").redirectErrorStream(true)
            .redirectOutput(File(tmp, "service.log")).start()
        try {
            val sock = File(tmp, "run/control.sock")
            repeat(200) { if (!sock.exists()) Thread.sleep(50) }
            check(sock.exists()) { File(tmp, "service.log").readText() }

            // Laptop: `phonelink touchpad pair --uri` prints the code as text too.
            val pair = proc(env, "phonelink", "touchpad", "pair", "--uri").start()
            val out = pair.inputStream.bufferedReader()
            val uri = generateSequence { out.readLine() }.first { it.startsWith("phonelink:touchpad?") }

            val events = LinkedBlockingQueue<PadEvent>()
            val phone = TouchpadLink.start("$tmp/phone/touchpad", "Kotlin phone", object : PadListener {
                override fun onEvent(event: PadEvent) { events.put(event) }
            })
            try {
                // A main EchoTwin code is refused with a clear message.
                val e = assertFailsWith<PadException> { phone.pair(uri.replace("phonelink:touchpad?", "phonelink:pair?")) }
                assertTrue(e.message!!.contains("phonelink touchpad pair"), e.message)

                val laptop = phone.pair(uri)
                assertEquals(1, phone.laptops().size)
                assertTrue(out.readText().contains("Paired with Kotlin phone"))
                events.await<PadEvent.Connected>()

                phone.openPad(70f, 150f)
                when (val status = events.awaitStatus()) {
                    is PadEvent.PadReady -> {}
                    is PadEvent.PadFailed -> assertTrue(status.message.contains("install-touchpad.sh"), status.message)
                    else -> fail("unexpected $status")
                }
                assertTrue(phone.button(true, true))
                assertTrue(phone.touch(listOf(Finger(0u, 1u, 0.5f, 0.5f))))
                assertTrue(phone.touch(emptyList()))
                assertTrue(phone.button(true, false))
                assertTrue(phone.scroll(-240))

                // Typing: the emoji is a run of its own (pasted by the app), the rest typed.
                val runs = dev.phonelink.core.textRuns("Hi 😀")
                assertEquals(listOf(true, false), runs.map { it.typeable })
                assertTrue(phone.typeText("Hello"))
                assertTrue(phone.key(dev.phonelink.core.PadKey.Char("c"), 1u))
                val typingOff = proc(env, "phonelink", "touchpad", "typing", "off").redirectErrorStream(true).start()
                check(typingOff.waitFor(15, TimeUnit.SECONDS) && typingOff.exitValue() == 0) { typingOff.inputStream.bufferedReader().readText() }
                assertTrue(phone.typeText("x"))
                events.await<PadEvent.KeyboardOff>()

                // Switched off on the laptop: the phone is told why.
                val off = proc(env, "phonelink", "touchpad", "off").redirectErrorStream(true).start()
                check(off.waitFor(15, TimeUnit.SECONDS) && off.exitValue() == 0) { off.inputStream.bufferedReader().readText() }
                val failed = events.await<PadEvent.PadFailed>()
                assertTrue(failed.message.contains("switched off"), failed.message)
                phone.closePad()
                assertTrue(phone.latencyMs() != null)

                phone.unpair(laptop.id)
                assertTrue(phone.laptops().isEmpty())
            } finally {
                phone.shutdown()
            }
        } finally {
            service.destroy()
            service.waitFor(5, TimeUnit.SECONDS)
            tmp.deleteRecursively()
        }
    }

    private fun LinkedBlockingQueue<PadEvent>.awaitStatus(): PadEvent {
        val deadline = System.currentTimeMillis() + 15_000
        while (true) {
            val left = deadline - System.currentTimeMillis()
            val e = poll(maxOf(left, 0), TimeUnit.MILLISECONDS) ?: fail("timed out waiting for the pad status")
            if (e is PadEvent.PadReady || e is PadEvent.PadFailed) return e
        }
    }
}
