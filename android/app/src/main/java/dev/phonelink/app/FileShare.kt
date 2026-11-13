package dev.phonelink.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.provider.OpenableColumns
import dev.phonelink.core.FileSource
import dev.phonelink.core.FileTransfer
import dev.phonelink.core.PhoneLink
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * Files between the phone and the computer. The Rust core does the transfer (resuming,
 * checking, cancelling); this side only:
 *
 * * opens the file being sent when the core asks (a content URI → file descriptor),
 * * publishes received files to Download/EchoTwin through MediaStore (no storage
 *   permission needed), and
 * * keeps the progress notification up to date.
 */
object FileShare : FileSource {
    private lateinit var app: Context

    /**
     * Files shared to EchoTwin from another app: Android's permission to read them ends
     * when the share screen closes, so they are opened right away and kept open until sent.
     */
    private val openShares = ConcurrentHashMap<String, ParcelFileDescriptor>()
    private val sourceOf = ConcurrentHashMap<String, String>() // transfer id → source

    fun init(context: Context) {
        app = context.applicationContext
    }

    // ------------------------------------------------------------------ the core asks for a file

    override fun open(transferId: String, source: String): Int {
        openShares[source]?.let { pfd ->
            return try {
                pfd.dup().detachFd()
            } catch (e: Exception) {
                -1
            }
        }
        // Picked in the app: the permission was kept (takePersistableUriPermission).
        return try {
            app.contentResolver.openFileDescriptor(Uri.parse(source), "r")?.detachFd() ?: -1
        } catch (e: Exception) {
            -1
        }
    }

    // ------------------------------------------------------------------ sending

    private data class Meta(val name: String, val size: Long, val mime: String)

    private fun meta(uri: Uri, pfd: ParcelFileDescriptor?): Meta? {
        val cr = app.contentResolver
        var name: String? = null
        var size = -1L
        try {
            cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val n = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    val s = c.getColumnIndex(OpenableColumns.SIZE)
                    if (n >= 0 && !c.isNull(n)) name = c.getString(n)
                    if (s >= 0 && !c.isNull(s)) size = c.getLong(s)
                }
            }
        } catch (e: Exception) {
            // Some providers don't answer queries; fall back below.
        }
        if (size < 0) size = pfd?.statSize ?: -1
        if (size < 0) return null
        val mime = cr.getType(uri) ?: "application/octet-stream"
        return Meta(name ?: uri.lastPathSegment ?: "file", size, mime)
    }

    /**
     * Queue files for the computer. `fromShare`: they came from another app's Share menu
     * (opened now and kept open, see [openShares]). Runs on the caller's thread: call from
     * a background thread. Returns a message for the user.
     */
    fun send(link: PhoneLink, uris: List<Uri>, fromShare: Boolean): String {
        var queued = 0
        var failed = 0
        for (uri in uris) {
            try {
                val pfd = if (fromShare) app.contentResolver.openFileDescriptor(uri, "r") else null
                val m = meta(uri, pfd)
                if (m == null) {
                    pfd?.close()
                    failed++
                    continue
                }
                val source = uri.toString()
                if (pfd != null) openShares.put(source, pfd)?.close()
                val id = link.sendFile(null, source, m.name, m.size.toULong(), m.mime)
                sourceOf[id] = source
                queued++
            } catch (e: Exception) {
                failed++
            }
        }
        val device = link.devices().firstOrNull()
        return when {
            queued == 0 -> "Couldn't read ${if (failed == 1) "that file" else "those files"}"
            device?.connected == true -> "Sending ${plural(queued)} to ${device.name}"
            else -> "${plural(queued).replaceFirstChar { it.uppercase() }} will be sent when your computer connects"
        } + if (failed > 0 && queued > 0) " (${plural(failed)} couldn't be read)" else ""
    }

    private fun plural(n: Int) = if (n == 1) "1 file" else "$n files"

    // ------------------------------------------------------------------ progress and results

    /** Called for every transfer change (on EchoTwin's background thread). */
    fun onChanged(t: FileTransfer, all: () -> List<FileTransfer>) {
        val finished = t.state == "done" || t.state == "failed" || t.state == "cancelled"
        if (finished && t.outgoing) {
            sourceOf.remove(t.id)?.let { src -> openShares.remove(src)?.close() }
            if (t.state == "failed") Notifications.fileResult(app, t.id, "Couldn't send ${t.name}", t.message, null)
        }
        if (!t.outgoing && t.state == "done") {
            val from = clipboardFrom.remove(t.id)
            if (from != null && Prefs.clipboardSharing(app)) toClipboard(t, from) else publish(t)
        }
        Notifications.transfers(app, all().filter { it.state == "active" || it.state == "waiting" || it.state == "queued" })
    }

    /** Transfers that are images copied on the computer (transfer id → computer name). */
    private val clipboardFrom = ConcurrentHashMap<String, String>()

    /** The computer said transfer [transferId] is an image it copied: clipboard, not Downloads. */
    fun expectClipboard(transferId: String, computer: String) {
        clipboardFrom[transferId] = computer
    }

    /**
     * Put a received image on the clipboard at full quality. It stays in the app's private
     * cache (the newest 5 are kept); apps you paste into may read just that file.
     */
    private fun toClipboard(t: FileTransfer, computer: String) {
        val private = t.path?.let(::File) ?: return
        if (!private.exists()) return
        val dir = File(app.cacheDir, "clipboard").apply { mkdirs() }
        dir.listFiles()?.sortedBy { it.lastModified() }?.dropLast(4)?.forEach { it.delete() }
        val file = File(dir, t.name.ifBlank { "image.png" })
        if (!private.renameTo(file)) {
            runCatching { private.copyTo(file, overwrite = true); private.delete() }.onFailure { return publish(t) }
        }
        val uri = androidx.core.content.FileProvider.getUriForFile(app, app.packageName + ".files", file)
        val clip = ClipData.newUri(app.contentResolver, "Image from $computer", uri)
        android.os.Handler(android.os.Looper.getMainLooper()).post {
            runCatching { app.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip) }
            LinkManager.toast("Image copied from $computer")
        }
    }

    /** At start-up: files that arrived while the app was being closed are published now. */
    fun publishLeftovers(list: List<FileTransfer>) {
        list.filter { !it.outgoing && it.state == "done" }.forEach(::publish)
    }

    /**
     * Move a received file from the app's private folder to Download/EchoTwin, where the
     * Files app and others see it. Idempotent: once moved, the private copy is gone.
     */
    @Synchronized
    private fun publish(t: FileTransfer) {
        val private = t.path?.let(::File) ?: return
        if (!private.exists()) return
        val cr = app.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, t.name)
            put(MediaStore.MediaColumns.MIME_TYPE, t.mime.ifBlank { "application/octet-stream" })
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/EchoTwin")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = try {
            cr.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
        } catch (e: Exception) {
            null
        } ?: return
        try {
            cr.openOutputStream(uri)?.use { out -> private.inputStream().use { it.copyTo(out) } }
                ?: throw IllegalStateException("no output")
            cr.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            private.delete()
            val open = Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, t.mime.ifBlank { "*/*" })
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            Notifications.fileResult(app, t.id, "Received ${t.name}", "From ${t.deviceName} · saved in Download/EchoTwin", open)
        } catch (e: Exception) {
            runCatching { cr.delete(uri, null, null) } // keep the private copy; retried later
        }
    }
}
