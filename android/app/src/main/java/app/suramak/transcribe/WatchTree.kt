package app.suramak.transcribe

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import org.json.JSONArray
import org.json.JSONObject
import java.io.InputStream

/**
 * M24A — the SAF side of the watched inbox.
 *
 * The scanner's rules (settle gate, extension, `.part` sibling, depth, bound,
 * sha256 dedupe) live in Rust and are host-tested against a fake source. This
 * object is the small platform half behind the same `WatchSource` trait: list
 * one tree document's children, open one document's stream. Only the content
 * resolver can walk a SAF tree, so the traversal belongs here.
 *
 * Every call is a `@JvmStatic` method on the `WatchTree` class, called from
 * `crates/suramak-android/src/watch_host.rs` (compiled by the NDK gate only).
 * A failure is a JSON `{"ok":false,"error":..}` — never an empty listing that
 * would look like an empty folder.
 */
object WatchTree {

    /**
     * Lists one level. `relative` is empty for the tree's root document;
     * otherwise it is a document id previously returned by this method.
     */
    @JvmStatic
    fun list(context: Context, tree: String, relative: String): String {
        return try {
            val treeUri = Uri.parse(tree)
            val documentId =
                if (relative.isEmpty()) {
                    DocumentsContract.getTreeDocumentId(treeUri)
                } else {
                    relative
                }
            val children =
                DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId)
            val resolver = context.contentResolver
            val cursor = resolver.query(
                children,
                arrayOf(
                    DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                    DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                    DocumentsContract.Document.COLUMN_MIME_TYPE,
                    DocumentsContract.Document.COLUMN_SIZE,
                    DocumentsContract.Document.COLUMN_LAST_MODIFIED,
                ),
                null,
                null,
                null,
            ) ?: return JSONObject()
                .put("ok", false)
                .put("error", "the folder could not be listed")
                .toString()
            val entries = JSONArray()
            cursor.use {
                while (it.moveToNext()) {
                    val id = it.getString(0) ?: continue
                    val name = it.getString(1) ?: continue
                    val mime = it.getString(2) ?: ""
                    entries.put(
                        JSONObject()
                            .put("id", id)
                            .put("name", name)
                            .put("dir", mime == DocumentsContract.Document.MIME_TYPE_DIR)
                            .put("size", if (it.isNull(3)) 0L else it.getLong(3))
                            .put("mtime", if (it.isNull(4)) 0L else it.getLong(4)),
                    )
                }
            }
            JSONObject().put("ok", true).put("entries", entries).toString()
        } catch (err: Exception) {
            JSONObject()
                .put("ok", false)
                .put("error", err.message ?: err.javaClass.simpleName)
                .toString()
        }
    }

    /**
     * Opens one document's bytes. `null` when the provider cannot (the Rust
     * reader treats it as a skipped file, not an empty one).
     */
    @JvmStatic
    fun open(context: Context, tree: String, documentId: String): InputStream? {
        return try {
            val treeUri = Uri.parse(tree)
            val document = DocumentsContract.buildDocumentUriUsingTree(treeUri, documentId)
            context.contentResolver.openInputStream(document)
        } catch (err: Exception) {
            null
        }
    }
}
