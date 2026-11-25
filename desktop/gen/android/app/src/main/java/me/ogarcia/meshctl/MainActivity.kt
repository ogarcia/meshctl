package me.ogarcia.meshctl

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.graphics.ColorUtils
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONObject

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
    open(intent)
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

  /** The panel's webview, once made. */
  private var webView: WebView? = null

  /** A link meshctl was opened with, for the panel to take once it loads. */
  @Volatile
  private var link: String? = null

  /** A conversation to open, from a notification, for the panel to take. */
  @Volatile
  private var chat: String? = null

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    webView.addJavascriptInterface(Panel(), "meshctlApp")
  }

  /**
   * A link to add what it holds, from a meshcore:// opened or text shared:
   * to the panel's /share, as a browser's share target has it, straight
   * away where it shows already, or taken by the panel as it loads.
   */
  private fun open(intent: Intent?) {
    if (intent?.action == OPEN_CHAT) {
      openChat(intent.getStringExtra(CHAT) ?: return)
      return
    }
    val opened = when (intent?.action) {
      Intent.ACTION_VIEW -> intent.dataString
      Intent.ACTION_SEND -> intent.getStringExtra(Intent.EXTRA_TEXT)
      else -> null
    } ?: return
    val shown = webView?.url?.let(Uri::parse)
    if (shown != null && (shown.scheme == "http" || shown.scheme == "https")) {
      webView?.loadUrl(
        shown.buildUpon()
          .path("/share")
          .clearQuery()
          .appendQueryParameter("text", opened)
          .fragment(null)
          .build()
          .toString()
      )
    } else {
      link = opened
    }
  }

  /**
   * A conversation to open, from a notification picked: told to the panel
   * straight away where it shows already, or taken by it once it is up.
   */
  private fun openChat(opened: String) {
    val shown = webView?.url?.let(Uri::parse)
    if (shown != null && (shown.scheme == "http" || shown.scheme == "https")) {
      val detail = JSONObject.quote(opened)
      webView?.evaluateJavascript(
        "window.dispatchEvent(new CustomEvent('meshctl-chat', { detail: $detail }))", null
      )
    } else {
      chat = opened
    }
  }

  /** What the panel and the app tell each other. */
  @Keep
  inner class Panel {
    /** The link meshctl was opened with, once; none, if not. */
    @Keep
    @JavascriptInterface
    fun takeLink(): String? {
      val taken = link
      link = null
      return taken
    }

    /** The conversation to open, from a notification, once; none, if not. */
    @Keep
    @JavascriptInterface
    fun takeChat(): String? {
      val taken = chat
      chat = null
      return taken
    }

    /**
     * The system's bars, in the colour of the panel's own at their edges,
     * as the panel says it, its theme picked or the system's changed: the
     * room left for them painted with it, their icons dark on light and
     * light on dark.
     */
    @Keep
    @JavascriptInterface
    fun paint(color: String) {
      val parsed = try {
        Color.parseColor(color.trim())
      } catch (_: IllegalArgumentException) {
        return
      }
      runOnUiThread {
        findViewById<View>(android.R.id.content).setBackgroundColor(parsed)
        val light = ColorUtils.calculateLuminance(parsed) > 0.5
        WindowCompat.getInsetsController(window, window.decorView).apply {
          isAppearanceLightStatusBars = light
          isAppearanceLightNavigationBars = light
        }
      }
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    ask(intent)
    open(intent)
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
    /** A notification's, to open its conversation, given as [CHAT]. */
    const val OPEN_CHAT = "me.ogarcia.meshctl.OPEN_CHAT"
    const val CHAT = "chat"
    private const val NOTIFICATIONS_ASKED = 7434
    private const val BLUETOOTH_ASKED = 7435
  }
}
