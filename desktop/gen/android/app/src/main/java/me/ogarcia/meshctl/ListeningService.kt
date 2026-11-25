package me.ogarcia.meshctl

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import androidx.annotation.Keep
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

/**
 * The daemon, in a process of its own: it runs as long as this service
 * does, apart from the app's window, which goes and comes back as Android
 * has it. Started as the window opens.
 *
 * In the foreground, with its notification, when the daemon says it is to
 * go on listening with the app closed; as any other service if not, which
 * Android stops a while after the app is left, and the daemon with it.
 */
class ListeningService : Service() {
  private var daemon: Thread? = null
  private val main = Handler(Looper.getMainLooper())

  @Volatile
  private var foreground = false

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    System.loadLibrary("meshctl_app")
    daemon = Thread({ runDaemon(filesDir.absolutePath) }, "meshctld").apply { start() }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    // Notifications just allowed: its own, posted before and unseen, again.
    if (intent?.action == REFRESH && foreground) {
      post()
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    stopDaemon()
    daemon?.join(STOPPING_MS)
    super.onDestroy()
  }

  /**
   * Called by the daemon, from its threads, as it starts and as it is set:
   * through JNI alone, kept from the shrinker, which sees no call to it.
   */
  @Keep
  fun keepListening(on: Boolean) {
    main.post {
      if (on) {
        post()
      } else if (foreground) {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        foreground = false
      }
    }
  }

  private fun post() {
    val manager = getSystemService(NotificationManager::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      manager.createNotificationChannel(
        NotificationChannel(
          CHANNEL, getString(R.string.listening_channel), NotificationManager.IMPORTANCE_LOW
        ).apply { setShowBadge(false) }
      )
    }
    val open = PendingIntent.getActivity(
      this,
      0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )
    val notification = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_listening)
      .setContentTitle(getString(R.string.app_name))
      .setContentText(getString(R.string.listening_text))
      .setContentIntent(open)
      .setOngoing(true)
      .setSilent(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .build()
    val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
    } else {
      0
    }
    try {
      ServiceCompat.startForeground(this, NOTIFICATION, notification, type)
      foreground = true
    } catch (error: Exception) {
      // Android lets an app start one only while it is in sight.
      android.util.Log.w("meshctl", "cannot listen in the foreground: $error")
    }
  }

  private external fun runDaemon(data: String)
  private external fun stopDaemon()

  companion object {
    /** Posts its notification again, if it is in the foreground. */
    const val REFRESH = "me.ogarcia.meshctl.REFRESH"
    private const val CHANNEL = "listening"
    private const val NOTIFICATION = 1
    private const val STOPPING_MS = 8000L
  }
}
