package me.ogarcia.meshctl

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // The daemon first, in its service's process, if it is not running
    // already: the window waits for it to serve.
    startService(Intent(this, ListeningService::class.java))
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Drawn from edge to edge, as Android has it now: the panel kept clear
    // of the status bar, the navigation bar and, when it shows, the
    // keyboard, which would otherwise lie over its heading and its tabs.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or
          WindowInsetsCompat.Type.displayCutout() or
          WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    ask(intent)
    // The notification that says it goes on listening, where Android asks
    // for leave to show it.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
      ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) !=
      PackageManager.PERMISSION_GRANTED
    ) {
      ActivityCompat.requestPermissions(
        this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), NOTIFICATIONS_ASKED
      )
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    ask(intent)
  }

  /** The service, looking for radios by Bluetooth, asks for leave to. */
  private fun ask(intent: Intent?) {
    if (intent?.action != ASK_BLUETOOTH) {
      return
    }
    val wanted = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else {
      arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }
    ActivityCompat.requestPermissions(this, wanted, BLUETOOTH_ASKED)
  }

  // Notifications allowed: the service's, if it posted one unseen, again.
  override fun onRequestPermissionsResult(
    requestCode: Int,
    permissions: Array<out String>,
    grantResults: IntArray
  ) {
    super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    if (requestCode == NOTIFICATIONS_ASKED &&
      grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
    ) {
      startService(
        Intent(this, ListeningService::class.java).setAction(ListeningService.REFRESH)
      )
    }
  }

  companion object {
    /** The service's, for leave to use Bluetooth. */
    const val ASK_BLUETOOTH = "me.ogarcia.meshctl.ASK_BLUETOOTH"
    private const val NOTIFICATIONS_ASKED = 7434
    private const val BLUETOOTH_ASKED = 7435
  }
}
