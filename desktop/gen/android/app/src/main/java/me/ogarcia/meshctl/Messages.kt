package me.ogarcia.meshctl

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import org.json.JSONException
import org.json.JSONObject

/**
 * Notifications of messages, as the daemon tells of them: one for each
 * conversation, with its latest messages and who wrote them, a channel's
 * as a group's, in a channel of Android's of their own. Gone as the
 * conversation is read, wherever; on one, the app opens on it.
 */
class Messages(private val context: Context) {
  /** Each conversation's latest messages notified of, by its key. */
  private val shown = HashMap<String, MutableList<NotificationCompat.MessagingStyle.Message>>()

  /** Shows a message, as the daemon gave it, in JSON. */
  fun show(json: String) {
    val notification = try {
      JSONObject(json)
    } catch (_: JSONException) {
      return
    }
    val radio = notification.optInt("radio")
    if (notification.optBoolean("contacts_full")) {
      full(radio, notification.optString("radio_name"))
      return
    }
    val peer = notification.optJSONObject("peer") ?: return
    val key = key(radio, peer)
    val channel = notification.optString("channel").takeIf { it.isNotEmpty() }
    val said = notification.optString("text").takeIf { it.isNotEmpty() }
    // A waypoint's: what happened to it, then its emoji and name.
    val waypoint = notification.optJSONObject("waypoint")
    val text = if (waypoint != null) {
      val happened = when (waypoint.optString("event")) {
        "entered" -> context.getString(R.string.waypoint_entered)
        "left" -> context.getString(R.string.waypoint_left)
        else -> context.getString(R.string.waypoint_shared)
      }
      when {
        said == null -> happened
        waypoint.optString("event") == "shared" -> "$happened: $said"
        else -> "$happened $said"
      }
    } else {
      said ?: context.getString(R.string.message_new)
    }
    val radioName = notification.optString("radio_name")
    val sender = Person.Builder().setName(notification.optString("sender")).build()

    val messages = shown.getOrPut(key) { mutableListOf() }
    messages += NotificationCompat.MessagingStyle.Message(text, System.currentTimeMillis(), sender)
    while (messages.size > KEPT) {
      messages.removeAt(0)
    }
    val me = Person.Builder()
      .setName(radioName.ifEmpty { context.getString(R.string.app_name) })
      .build()
    val style = NotificationCompat.MessagingStyle(me)
      .setConversationTitle(channel?.let { "#$it" })
      .setGroupConversation(channel != null)
    messages.forEach { style.addMessage(it) }

    val chat = JSONObject().put("radio", radio).put("peer", peer)
      .apply { waypoint?.let { put("waypoint", it.optLong("id")) } }
      .toString()
    val open = PendingIntent.getActivity(
      context,
      key.hashCode(),
      Intent(context, MainActivity::class.java)
        .setAction(MainActivity.OPEN_CHAT)
        .putExtra(MainActivity.CHAT, chat)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )
    channel()
    val built = NotificationCompat.Builder(context, CHANNEL)
      .setSmallIcon(R.drawable.ic_listening)
      .setStyle(style)
      .setSubText(radioName.ifEmpty { null })
      .setContentIntent(open)
      .setAutoCancel(true)
      .setCategory(NotificationCompat.CATEGORY_MESSAGE)
      .setPriority(NotificationCompat.PRIORITY_HIGH)
      .build()
    try {
      NotificationManagerCompat.from(context).notify(key, ID, built)
    } catch (_: SecurityException) {
      // Not let show notifications: nothing shown.
    }
  }

  /**
   * A MeshCore radio with no room for the nodes it hears: one notification
   * of its own, by its name; on it, the app opens on the radio's settings.
   */
  private fun full(radio: Int, radioName: String) {
    val key = "$radio-full"
    val chat = JSONObject().put("radio", radio)
      .put("peer", JSONObject().put("kind", "channel").put("channel", 0))
      .put("full", true)
      .toString()
    val open = PendingIntent.getActivity(
      context,
      key.hashCode(),
      Intent(context, MainActivity::class.java)
        .setAction(MainActivity.OPEN_CHAT)
        .putExtra(MainActivity.CHAT, chat)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )
    channel()
    val said = context.getString(R.string.contacts_full)
    val built = NotificationCompat.Builder(context, CHANNEL)
      .setSmallIcon(R.drawable.ic_listening)
      .setContentTitle(radioName.ifEmpty { context.getString(R.string.app_name) })
      .setContentText(said)
      .setStyle(NotificationCompat.BigTextStyle().bigText(said))
      .setContentIntent(open)
      .setAutoCancel(true)
      .setCategory(NotificationCompat.CATEGORY_STATUS)
      .setPriority(NotificationCompat.PRIORITY_HIGH)
      .build()
    try {
      NotificationManagerCompat.from(context).notify(key, ID, built)
    } catch (_: SecurityException) {
      // Not let show notifications: nothing shown.
    }
  }

  /** A conversation read: its notification goes, and what it showed. */
  fun read(radio: Int, peer: String) {
    val key = try {
      key(radio, JSONObject(peer))
    } catch (_: JSONException) {
      return
    }
    shown.remove(key)
    NotificationManagerCompat.from(context).cancel(key, ID)
  }

  private fun channel() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      context.getSystemService(NotificationManager::class.java).createNotificationChannel(
        NotificationChannel(
          CHANNEL, context.getString(R.string.messages_channel), NotificationManager.IMPORTANCE_HIGH
        )
      )
    }
  }

  /** A conversation's key: its radio, and its channel or its node. */
  private fun key(radio: Int, peer: JSONObject): String =
    when (peer.optString("kind")) {
      "channel" -> "$radio-c${peer.optLong("channel")}"
      else -> "$radio-n${peer.optLong("node")}"
    }

  companion object {
    private const val CHANNEL = "messages"
    private const val ID = 2
    /** The most messages a conversation's notification shows. */
    private const val KEPT = 6
  }
}
