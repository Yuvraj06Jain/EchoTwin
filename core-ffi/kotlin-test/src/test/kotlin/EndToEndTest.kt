import dev.phonelink.core.EventListener
import dev.phonelink.core.LinkEvent
import dev.phonelink.core.LinkException
import dev.phonelink.core.PhoneLink
import java.io.File
import java.nio.file.Files
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.test.fail

class EndToEndTest {
    private val bin = System.getenv("PHONELINK_BIN_DIR") ?: error("PHONELINK_BIN_DIR not set")

    private fun proc(env: Map<String, String>, vararg args: String): ProcessBuilder =
        ProcessBuilder(listOf("$bin/${args[0]}") + args.drop(1)).apply { environment().putAll(env) }

    private fun run(env: Map<String, String>, vararg args: String): String {
        val p = proc(env, *args).redirectErrorStream(true).start()
        val out = p.inputStream.bufferedReader().readText()
        check(p.waitFor(15, TimeUnit.SECONDS) && p.exitValue() == 0) { "${args.joinToString(" ")} failed: $out" }
        return out
    }

    private inline fun <reified T : LinkEvent> LinkedBlockingQueue<LinkEvent>.await(): T {
        val deadline = System.currentTimeMillis() + 15_000
        while (true) {
            val left = deadline - System.currentTimeMillis()
            val e = poll(maxOf(left, 0), TimeUnit.MILLISECONDS) ?: fail("timed out waiting for ${T::class.simpleName}")
            if (e is T) return e
        }
    }

    @Test
    fun kotlinPhonePairsAndTalksWithTheRealDaemon() {
        val tmp = Files.createTempDirectory("phonelink").toFile()
        val env = mapOf(
            "PHONELINK_DATA_DIR" to "$tmp/laptop",
            "PHONELINK_SOCKET" to "$tmp/pl.sock",
            "PHONELINK_CONFIG" to "$tmp/config.toml",
        )
        File(tmp, "config.toml").writeText("port = 0\nnotifications = false\ndiscovery = false\ndevice_name = \"Kubuntu\"\n")
        val daemon = proc(env, "phonelinkd").redirectErrorStream(true).redirectOutput(File(tmp, "daemon.log")).start()
        try {
            val sock = File(tmp, "pl.sock")
            repeat(100) { if (!sock.exists()) Thread.sleep(50) }
            check(sock.exists()) { File(tmp, "daemon.log").readText() }

            // Laptop: `phonelink pair` (JSON mode); first line carries the QR payload.
            val pair = proc(env, "phonelink", "--json", "pair", "--ttl", "60").start()
            val started = pair.inputStream.bufferedReader().readLine()
            val uri = Regex("\"uri\":\"([^\"]+)\"").find(started)!!.groupValues[1]

            // Phone: exactly what the Android app does.
            val events = LinkedBlockingQueue<LinkEvent>()
            val phone = PhoneLink.start("$tmp/phone", "Galaxy S24 FE", object : EventListener {
                override fun onEvent(event: LinkEvent) { events.put(event) }
            })
            val code = phone.readPairingCode(uri)
            assertEquals("Kubuntu", code.name)
            val device = phone.pair(uri)
            assertEquals(code.id, device.id)
            events.await<LinkEvent.Connected>()
            assertTrue(pair.waitFor(10, TimeUnit.SECONDS), "CLI should finish after pairing")

            run(env, "phonelink", "send-text", "hello from kubuntu")
            assertEquals("hello from kubuntu", events.await<LinkEvent.Text>().text)

            assertTrue(phone.sendText(null, "hi from the phone"))
            val devices = run(env, "phonelink", "--json", "devices")
            assertTrue("Galaxy S24 FE" in devices && "\"connected\":true" in devices, devices)

            assertFailsWith<LinkException> { phone.readPairingCode("not a pairing code") }
            assertFailsWith<LinkException> { phone.sendText("0000-0000-0000-0000", "x") }

            phone.unpair(device.id)
            assertTrue(phone.devices().isEmpty())
            phone.shutdown()
        } finally {
            daemon.destroy()
            daemon.waitFor(5, TimeUnit.SECONDS)
        }
    }
}
