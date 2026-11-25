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

  private companion object {
    const val NOTIFICATIONS_ASKED = 7434
  }
}
