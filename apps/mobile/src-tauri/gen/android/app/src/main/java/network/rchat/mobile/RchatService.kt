package network.rchat.mobile

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import com.chaquo.python.Python

/**
 * Keeps the Reticulum node alive while the app is in the background.
 *
 * The service owns the engine process lifetime, and the order in [onCreate]
 * matters: the engine starts first and binds a loopback bridge port, then the
 * embedded Python bridge dials that port. The waits and locks are what make LAN
 * discovery work with the screen off; Android otherwise suspends Wi-Fi multicast.
 */
class RchatService : Service() {
  private var wifiLock: WifiManager.WifiLock? = null
  private var multicastLock: WifiManager.MulticastLock? = null
  private var bridgeThread: Thread? = null

  override fun onCreate() {
    super.onCreate()
    EngineBridge.init(this)
    Notifications.createChannels(this)
    // Before the service notification exists: a sticky restart must not inherit
    // threads the previous process left in the shade.
    Notifications.cancelAll(this)
    startInForeground()

    val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    wifiLock =
      wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "rchat:wifi").apply {
        setReferenceCounted(false)
        acquire()
      }
    multicastLock =
      wifi.createMulticastLock("rchat:multicast").apply {
        setReferenceCounted(false)
        acquire()
      }

    startEngine()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    // The activity may start us again after a rotation or a task switch; the
    // engine start below is idempotent and returns the running bridge port.
    if (bridgeThread?.isAlive != true) startEngine()
    return START_STICKY
  }

  private fun startInForeground() {
    val notification = Notifications.serviceNotification(this)
    try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
        startForeground(
          Notifications.SERVICE_ID,
          notification,
          ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING,
        )
      } else {
        startForeground(Notifications.SERVICE_ID, notification)
      }
    } catch (error: Throwable) {
      // Android 12+ can refuse a foreground promotion from the background. The
      // engine is still worth running; just record why the notification is gone.
      Log.e(TAG, "could not enter the foreground", error)
    }
  }

  private fun startEngine() {
    val engineDir = EngineBridge.engineDir(this).apply { mkdirs() }
    val bridgeDir = EngineBridge.bridgeDir(this).apply { mkdirs() }

    val port = EngineBridge.nativeStart(engineDir.absolutePath, bridgeDir.absolutePath)
    if (port <= 0) {
      Log.e(TAG, "the engine did not start; see the boot error in the UI")
      return
    }
    val displayName = EngineBridge.nativeDisplayName()
    bridgeThread =
      Thread(
          {
            try {
              Python.getInstance()
                .getModule("bridge")
                .callAttr("main", port, bridgeDir.absolutePath, displayName, true)
            } catch (error: Throwable) {
              Log.e(TAG, "the Python bridge stopped", error)
            }
          },
          "rchat-python",
        )
        .apply {
          isDaemon = true
          start()
        }
  }

  override fun onDestroy() {
    bridgeThread = null
    EngineBridge.nativeStop()
    // The threads the engine accumulated are gone with it, so nothing may stay
    // in the shade claiming otherwise.
    Notifications.clearMessages(this)
    multicastLock?.release()
    wifiLock?.release()
    multicastLock = null
    wifiLock = null
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private companion object {
    const val TAG = "rchat"
  }
}
