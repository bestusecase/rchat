package network.rchat.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build

/**
 * Notification channels and payloads.
 *
 * Two channels with different purposes: the foreground-service channel is quiet
 * and ongoing, while the messages channel is allowed to alert without making a
 * sound and never carries message text. Message contents stay in the webview.
 */
object Notifications {
  const val SERVICE_CHANNEL = "rchat-service"
  const val MESSAGE_CHANNEL = "rchat-messages"
  const val SERVICE_ID = 1
  const val MESSAGE_ID = 2

  fun createChannels(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java) ?: return

    val service =
      NotificationChannel(
        SERVICE_CHANNEL,
        "Background engine",
        NotificationManager.IMPORTANCE_LOW,
      ).apply {
        description = "Keeps rchat connected to Reticulum"
        setShowBadge(false)
      }
    manager.createNotificationChannel(service)

    val messages =
      NotificationChannel(
        MESSAGE_CHANNEL,
        "Messages",
        NotificationManager.IMPORTANCE_DEFAULT,
      ).apply {
        description = "New messages arrived while rchat was in the background"
        setSound(null, null)
        enableVibration(false)
        enableLights(false)
        setShowBadge(true)
      }
    manager.createNotificationChannel(messages)
  }

  fun serviceNotification(context: Context): Notification =
    Notification.Builder(context, SERVICE_CHANNEL)
      .setSmallIcon(android.R.drawable.stat_notify_chat)
      .setContentTitle(context.getString(R.string.app_name))
      .setContentText("Connected over Reticulum")
      .setContentIntent(openApp(context))
      .setOngoing(true)
      .setShowWhen(false)
      .setCategory(Notification.CATEGORY_SERVICE)
      .build()

  fun postMessages(context: Context, count: Int) {
    if (!canPost(context)) return
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    val summary = if (count == 1) "1 new message" else "$count new messages"
    val notification =
      Notification.Builder(context, MESSAGE_CHANNEL)
        .setSmallIcon(android.R.drawable.stat_notify_chat)
        .setContentTitle(context.getString(R.string.app_name))
        .setContentText(summary)
        .setNumber(count)
        .setAutoCancel(true)
        .setOnlyAlertOnce(true)
        .setShowWhen(false)
        .setCategory(Notification.CATEGORY_MESSAGE)
        // Nothing here is private: the body never leaves the app. Private
        // visibility would hide the summary entirely on a secure lock screen,
        // which is exactly where it is most useful.
        .setVisibility(Notification.VISIBILITY_PUBLIC)
        .setContentIntent(openApp(context))
        .build()
    manager.notify(MESSAGE_ID, notification)
  }

  fun clearMessages(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    manager.cancel(MESSAGE_ID)
  }

  /** Android 13+ gates notifications behind a runtime permission. */
  fun canPost(context: Context): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
      context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED

  private fun openApp(context: Context): PendingIntent {
    val intent =
      Intent(context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
    return PendingIntent.getActivity(
      context,
      0,
      intent,
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
  }
}
