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
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import org.json.JSONObject

/**
 * Notification channels and payloads.
 *
 * Two channels with different purposes: the foreground-service channel is quiet
 * and ongoing, while the messages channel is allowed to alert without making a
 * sound. A message notification is a conversation thread: one child per
 * conversation that has unread messages, grouped under a summary that counts
 * them. The content is private, so a locked screen shows only the count.
 *
 * Everything rendered here comes from the snapshot Rust sends; the only state
 * kept on this side is the set of child ids currently posted and the id
 * allocated to each conversation, so a conversation that disappears from the
 * snapshot can be cancelled.
 */
object Notifications {
  const val SERVICE_CHANNEL = "rchat-service"
  const val MESSAGE_CHANNEL = "rchat-messages"
  const val SERVICE_ID = 1

  /** The summary keeps the id the old count-only notification used. */
  const val SUMMARY_ID = 2

  private const val MESSAGE_GROUP = "rchat-messages"
  private const val SUMMARY_LINES = 5

  /** Child ids start above the reserved service and summary ids. */
  private const val CHILD_ID_BASE = 3

  /** Child notifications in the shade right now; cancelled when they go stale. */
  private val postedChildren = mutableSetOf<Int>()

  /**
   * The child id allocated to each conversation.
   *
   * An id is handed out once and kept until the bookkeeping is fully cleared:
   * cancelling a conversation's notification does not release its id, so a
   * later conversation never inherits an id the shade may still remember. Only
   * [forgetThreads] empties the map, and a new process starts empty because the
   * service cancels everything before it promotes itself.
   */
  private val childIds = mutableMapOf<String, Int>()

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

    // Unchanged on purpose: channel importance is effectively permanent, and a
    // new channel id would surprise everyone who already has this one muted.
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

  /**
   * Render a snapshot of the conversations waiting in the background.
   *
   * The payload is the whole truth: children for conversations it does not
   * mention are cancelled, which is how reading or muting one conversation
   * clears it.
   */
  @Synchronized
  fun postThreads(context: Context, payload: String) {
    if (!canPost(context)) return
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    val snapshot = JSONObject(payload)
    val conversations = snapshot.getJSONArray("conversations")
    val conversationCount = snapshot.getInt("conversationCount")
    val unreadCount = snapshot.getInt("unreadCount")
    if (conversationCount == 0 || conversations.length() == 0) {
      clearMessages(context)
      return
    }

    val people = mutableMapOf<String, Person>()
    val summaryLines = NotificationCompat.InboxStyle()
    val live = mutableSetOf<Int>()
    // The snapshot is newest first, which is also the order the summary lists.
    for (index in 0 until conversations.length()) {
      val conversation = conversations.getJSONObject(index)
      val id = conversation.getString("id")
      val title = conversation.getString("title")
      val unread = conversation.getInt("unread")
      val messages = conversation.getJSONArray("messages")

      val style = NotificationCompat.MessagingStyle(self(context))
      style.setConversationTitle(title)
      for (position in 0 until messages.length()) {
        val message = messages.getJSONObject(position)
        style.addMessage(
          message.getString("body"),
          message.getLong("ts"),
          person(people, message.getString("senderId"), message.getString("senderLabel")),
        )
      }
      val latest =
        if (messages.length() == 0) ""
        else messages.getJSONObject(messages.length() - 1).getString("body")
      // One line per conversation, counted by position in the snapshot: a
      // shared child id must not be able to sneak in a sixth line.
      if (index < SUMMARY_LINES) summaryLines.addLine("$title — $latest")

      val child = childId(id)
      live += child
      manager.notify(
        child,
        threadNotification(context, title, unread, style, countOnly(context, unread)),
      )
    }

    // Children for conversations the snapshot no longer has: they were read,
    // muted, or trimmed away, so their notification is a lie now.
    for (stale in postedChildren - live) manager.cancel(stale)
    postedChildren.clear()
    postedChildren.addAll(live)

    val summaryText =
      "${plural(conversationCount, "conversation")} · ${plural(unreadCount, "new message")}"
    manager.notify(
      SUMMARY_ID,
      summaryNotification(context, summaryText, unreadCount, summaryLines),
    )
  }

  /**
   * Drop the summary and every thread.
   *
   * Called when the user can see the app again and when the service is
   * destroyed. Starting the service calls [cancelAll] instead, because a sticky
   * restart must not leave threads behind that the new process cannot explain.
   */
  @Synchronized
  fun clearMessages(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    manager.cancel(SUMMARY_ID)
    for (id in postedChildren) manager.cancel(id)
    forgetThreads()
  }

  /** Android 13+ gates notifications behind a runtime permission. */
  fun canPost(context: Context): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
      context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED

  /**
   * Cancel every notification the app has, including ones this process never
   * posted.
   *
   * The service calls this while starting: after a sticky restart the new
   * process cannot account for the threads the dead one left behind, so they
   * must not survive to be mistaken for current ones.
   */
  @Synchronized
  fun cancelAll(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    manager.cancelAll()
    forgetThreads()
  }

  /**
   * Forget both pieces of notification bookkeeping through one path.
   *
   * Every full clear — the foreground clear and the cancel-everything path —
   * goes through here, so a future third piece cannot be cleared on one path
   * and forgotten on the other.
   */
  private fun forgetThreads() {
    postedChildren.clear()
    childIds.clear()
  }

  /** Allocate a conversation's child id, above the reserved service and summary ids. */
  private fun childId(conversationId: String): Int {
    childIds[conversationId]?.let { return it }
    var id = CHILD_ID_BASE
    while (id in childIds.values) id++
    childIds[conversationId] = id
    return id
  }

  private fun plural(count: Int, singular: String): String =
    if (count == 1) "1 $singular" else "$count ${singular}s"

  private fun self(context: Context): Person =
    Person.Builder().setName(context.getString(R.string.app_name)).setKey("self").build()

  private fun person(known: MutableMap<String, Person>, id: String, label: String): Person =
    known.getOrPut(id) { Person.Builder().setName(label).setKey(id).build() }

  /** The chrome every message notification shares. */
  private fun chrome(context: Context, publicVersion: Notification): NotificationCompat.Builder =
    NotificationCompat.Builder(context, MESSAGE_CHANNEL)
      .setSmallIcon(android.R.drawable.stat_notify_chat)
      .setAutoCancel(true)
      .setOnlyAlertOnce(true)
      .setShowWhen(false)
      .setCategory(NotificationCompat.CATEGORY_MESSAGE)
      .setContentIntent(openApp(context))
      // Private keeps the text off a secure lock screen while still letting the
      // system's own "hide sensitive content" preference take effect; an
      // always-public notification would override that preference.
      .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
      .setPublicVersion(publicVersion)

  /** One conversation: a thread, collapsed to its newest message. */
  private fun threadNotification(
    context: Context,
    title: String,
    unread: Int,
    style: NotificationCompat.MessagingStyle,
    publicVersion: Notification,
  ): Notification =
    chrome(context, publicVersion)
      .setContentTitle(title)
      .setStyle(style)
      .setNumber(unread)
      .setGroup(MESSAGE_GROUP)
      .build()

  private fun summaryNotification(
    context: Context,
    text: String,
    unreadCount: Int,
    lines: NotificationCompat.InboxStyle,
  ): Notification =
    chrome(context, countOnly(context, unreadCount))
      .setContentTitle(context.getString(R.string.app_name))
      .setContentText(text)
      .setStyle(lines)
      // The total, not one conversation's count: it is what the launcher badge
      // reads, and the summary spans every conversation.
      .setNumber(unreadCount)
      .setGroup(MESSAGE_GROUP)
      .setGroupSummary(true)
      .build()

  /** What a locked screen is allowed to show: a count, and nothing else. */
  private fun countOnly(context: Context, count: Int): Notification =
    NotificationCompat.Builder(context, MESSAGE_CHANNEL)
      .setSmallIcon(android.R.drawable.stat_notify_chat)
      .setContentTitle(context.getString(R.string.app_name))
      .setContentText(plural(count, "new message"))
      .setNumber(count)
      .setOnlyAlertOnce(true)
      .setShowWhen(false)
      .setCategory(NotificationCompat.CATEGORY_MESSAGE)
      .setContentIntent(openApp(context))
      .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
      .build()

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
