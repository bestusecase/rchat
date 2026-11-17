package network.rchat.mobile

import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import android.util.Log

/**
 * The Java side of the native shell.
 *
 * Kotlin starts and stops the engine; Rust calls back for logging, notifications
 * and the battery settings page. Keep this object thin: the service owns the
 * lifecycle, [Notifications] owns the notification channels, and the engine
 * owns everything else.
 */
object EngineBridge {
  private const val LIBRARY = "rchat_mobile_lib"
  private const val TAG = "rchat"

  @Volatile private var appContext: Context? = null

  /**
   * Whether one of our activities is resumed.
   *
   * Not the same as visible: the screen can be off, or the keyguard can cover
   * the activity without pausing it. [userCanSeeApp] pairs this with the
   * display state instead of trusting it alone.
   */
  @Volatile private var activityVisible = false

  fun setActivityVisible(visible: Boolean) {
    activityVisible = visible
  }

  /**
   * Load the native library and remember the application context.
   *
   * Must run before any native call, and before Rust can call back into
   * [openBackgroundSettings] or the notification helpers. Safe to call again:
   * `System.loadLibrary` is idempotent.
   */
  fun init(context: Context) {
    appContext = context.applicationContext
    System.loadLibrary(LIBRARY)
  }

  /** Engine data directory; the Reticulum bridge gets a sibling directory. */
  fun engineDir(context: Context) = java.io.File(context.filesDir, "engine")

  /** Where the Python bridge keeps its identity, config and LXMF storage. */
  fun bridgeDir(context: Context) = java.io.File(context.filesDir, "rns")

  /** Start the engine; returns the loopback bridge port, or -1 on failure. */
  @JvmStatic external fun nativeStart(dataDir: String): Int

  @JvmStatic external fun nativeStop()

  /** The LXMF display name the bridge should announce. */
  @JvmStatic external fun nativeDisplayName(): String

  /** Forget the accumulated notification threads. Called on coming back. */
  @JvmStatic external fun nativeForgetThreads()

  /** Open the system page where the user can allow background battery use. */
  @JvmStatic
  fun openBackgroundSettings() {
    val context = appContext ?: return
    val intent =
      Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
        .setData(Uri.parse("package:" + context.packageName))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    context.startActivity(intent)
  }

  /**
   * Render a snapshot of the conversations waiting in the background.
   *
   * Rendering only: the policy of whether a snapshot should be posted, and what
   * a refusal forgets, is decided by Rust's delivery step before this is
   * called. A missing context, or the platform's notification permission,
   * still posts nothing.
   */
  @JvmStatic
  fun notifyThreads(payload: String) {
    val context = appContext ?: return
    Notifications.postThreads(context, payload)
  }

  /**
   * Write one attachment into the user's Downloads.
   *
   * Returns an empty string on success and the platform's reason otherwise, so
   * Rust can show a legible failure instead of a silent no-op. Called from
   * [save_attachment] in the engine shell, which owns the bytes.
   */
  @JvmStatic
  fun saveAttachment(name: String, data: ByteArray): String {
    val context = appContext ?: return "rchat is not ready to save files yet."
    return AttachmentStore.save(context, name, data)
  }

  @JvmStatic
  fun clearActivity() {
    // Drop what the engine accumulated as well as what is in the shade: the
    // user is reading these messages now, so they must not come back in the
    // next background notification.
    nativeForgetThreads()
    val context = appContext ?: return
    Notifications.clearMessages(context)
  }

  /**
   * One definition of "the user can see the app".
   *
   * Rust asks before it commits an incoming message and again before it posts,
   * so neither side has to re-derive the answer. Fails closed: before the
   * bridge has a context it cannot tell, so it says visible — nothing
   * accumulates and no message text is posted on a guess.
   */
  @JvmStatic
  fun userCanSeeApp(): Boolean {
    val context = appContext
    if (context == null) {
      // Cannot answer, which is not the same as "nobody is looking": count as
      // visible so nothing accumulates and no text is posted on a guess. Log it
      // so a device pass can tell this from a correctly suppressed post.
      Log.w(TAG, "cannot tell whether the user can see the app: no context yet")
      return true
    }
    val power = context.getSystemService(PowerManager::class.java)
    val keyguard = context.getSystemService(KeyguardManager::class.java)
    return activityVisible &&
      power?.isInteractive == true &&
      keyguard?.isKeyguardLocked != true
  }
}
