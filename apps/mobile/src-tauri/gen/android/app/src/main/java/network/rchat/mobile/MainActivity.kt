package network.rchat.mobile

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * The app window. The engine does not live here: the foreground service owns it
 * so it survives the activity, and this class only makes sure the service is
 * running, asks for the notification permission the service needs, and tells
 * the engine bridge whether the app is on screen.
 */
class MainActivity : TauriActivity() {
  private val requestNotifications =
    registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
      if (!granted) Log.i("rchat", "notifications are disabled; background messages will be silent")
    }

  // Android 15+ draws apps edge to edge whether they ask or not, so the system
  // bar and keyboard insets are applied to the content view here instead of
  // letting the webview sit under the status bar.
  override val handleBackNavigation: Boolean = true

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    applyWindowInsets()

    EngineBridge.init(applicationContext)
    startEngineService()
    requestNotificationPermission()
  }

  override fun onResume() {
    super.onResume()
    // Rust posts message notifications while no activity is on screen; coming
    // back means the user can see them, so the shade summary is spent.
    EngineBridge.setActivityVisible(true)
    EngineBridge.clearActivity()
  }

  override fun onPause() {
    EngineBridge.setActivityVisible(false)
    super.onPause()
  }

  private fun applyWindowInsets() {
    val content = findViewById<ViewGroup>(android.R.id.content) ?: return
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars =
        insets.getInsets(
          WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
        )
      val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
      view.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, ime.bottom))
      insets
    }
    ViewCompat.requestApplyInsets(content)
  }

  private fun startEngineService() {
    try {
      startForegroundService(Intent(this, RchatService::class.java))
    } catch (error: Throwable) {
      Log.e("rchat", "could not start the engine service", error)
    }
  }

  private fun requestNotificationPermission() {
    if (
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
          PackageManager.PERMISSION_GRANTED
    ) {
      requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
  }
}
