package network.rchat.mobile

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import java.io.File
import java.io.IOException

/**
 * Writes an attachment into a place the user can find it again.
 *
 * On Android 10 and later this is the Downloads collection, which needs no
 * permission and makes the file appear in Files and in gallery apps. Older
 * releases have no Downloads collection to insert into, so the app's own
 * external Downloads folder is the fallback: still a real file, without asking
 * for broad storage access just to save one attachment.
 */
internal object AttachmentStore {
  /**
   * Save [data] as [name]; returns "" on success and the reason on failure, so
   * the conversation can report a denied permission or a full disk as it is.
   */
  fun save(context: Context, name: String, data: ByteArray): String {
    val fileName = sanitize(name)
    return try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        saveToDownloads(context, fileName, data)
      } else {
        saveToAppDownloads(context, fileName, data)
      }
      ""
    } catch (err: Exception) {
      "Could not save $fileName: ${err.message ?: err.javaClass.simpleName}"
    }
  }

  private fun saveToDownloads(context: Context, name: String, data: ByteArray) {
    val resolver = context.contentResolver
    val values =
      ContentValues().apply {
        put(MediaStore.Downloads.DISPLAY_NAME, name)
        put(MediaStore.Downloads.MIME_TYPE, mimeType(name))
        put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
        put(MediaStore.Downloads.IS_PENDING, 1)
      }
    val uri =
      resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
        ?: throw IOException("the media store refused a new entry")
    try {
      resolver.openOutputStream(uri)?.use { stream -> stream.write(data) }
        ?: throw IOException("the media store gave no stream")
      values.clear()
      values.put(MediaStore.Downloads.IS_PENDING, 0)
      resolver.update(uri, values, null, null)
    } catch (err: Exception) {
      // A pending entry that never completed would show up as a broken file.
      runCatching { resolver.delete(uri, null, null) }
      throw err
    }
  }

  private fun saveToAppDownloads(context: Context, name: String, data: ByteArray) {
    val dir = context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
      ?: throw IOException("external storage is unavailable")
    if (!dir.exists() && !dir.mkdirs()) throw IOException("could not create ${dir.path}")
    var target = File(dir, name)
    var index = 1
    while (target.exists()) {
      target = File(dir, uniqueName(name, index))
      index += 1
    }
    target.writeBytes(data)
  }

  /** One path component: no separators, no traversal, never empty. */
  private fun sanitize(name: String): String {
    val base = name.substringAfterLast('/').substringAfterLast('\\')
    val cleaned = base.filter { !it.isISOControl() }.trim().trim('.')
    return if (cleaned.isEmpty()) "attachment" else cleaned.take(120)
  }

  private fun uniqueName(name: String, index: Int): String {
    val dot = name.lastIndexOf('.')
    return if (dot <= 0) "$name ($index)"
    else "${name.substring(0, dot)} ($index)${name.substring(dot)}"
  }

  private fun mimeType(name: String): String {
    val extension = name.substringAfterLast('.', "").lowercase()
    if (extension.isEmpty()) return "application/octet-stream"
    return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
      ?: "application/octet-stream"
  }
}
