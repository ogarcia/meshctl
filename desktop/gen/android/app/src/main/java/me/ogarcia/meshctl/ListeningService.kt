package me.ogarcia.meshctl

import android.app.NotificationChannel
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbManager
import com.hoho.android.usbserial.driver.CdcAcmSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialPort
import com.hoho.android.usbserial.driver.UsbSerialProber
import com.hoho.android.usbserial.util.SerialInputOutputManager
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
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
    usb.keys.forEach { close(it) }
    ble.closeAll()
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

  /** Messages notified of, one notification for each conversation. */
  private val messages by lazy { Messages(this) }

  /** Called by the daemon, as a message is to be notified of: in JSON. */
  @Keep
  fun notifyMessage(json: String) {
    main.post { messages.show(json) }
  }

  /** Called by the daemon, as a conversation is read, its peer in JSON. */
  @Keep
  fun readConversation(radio: Int, peer: String) {
    main.post { messages.read(radio, peer) }
  }

  /** A radio open by USB, and what reads it. */
  private class Opened(val port: UsbSerialPort, val reader: SerialInputOutputManager)

  private val usb = ConcurrentHashMap<Int, Opened>()

  /** Radios by Bluetooth, their payloads to and from the daemon. */
  private val ble by lazy {
    BleRadios(this, { handle, data -> onBleData(handle, data) }, { onBleClosed(it) })
  }

  /** The radios heard nearby, as JSON; empty while the user is asked. */
  @Keep
  fun lookBle(): String = ble.look() ?: ASKED_BLE

  /** Reaches a radio by Bluetooth: its handle, or a negative number. */
  @Keep
  fun openBle(address: String, meshcore: Boolean): Int = ble.open(address, meshcore)

  @Keep
  fun writeBle(handle: Int, data: ByteArray): Boolean = ble.write(handle, data)

  @Keep
  fun closeBle(handle: Int) = ble.close(handle)
  private val handles = AtomicInteger(0)

  /**
   * Every radio plugged in by USB with a driver for its serial chip: those
   * known by their ids, and any other that says it is a modem (CDC), as
   * boards with USB of their own are, nRF52's and ESP32-S3's.
   */
  private fun plugged(manager: UsbManager): List<UsbSerialDriver> {
    val known = UsbSerialProber.getDefaultProber().findAllDrivers(manager)
    val modems = manager.deviceList.values
      .filter { device -> known.none { it.device == device } }
      .filter { device ->
        (0 until device.interfaceCount).any {
          device.getInterface(it).interfaceClass == UsbConstants.USB_CLASS_COMM
        }
      }
      .map { CdcAcmSerialDriver(it) }
    return known + modems
  }

  /**
   * Opens a radio by USB for the daemon: the one with these ids, as
   * `vvvv:pppp`, or the first there is. Its handle; or NO_DEVICE, or ASKED
   * when Android is asking the user for leave to use it, or FAILED.
   */
  @Keep
  fun openUsb(device: String, baud: Int): Int {
    val manager = getSystemService(UsbManager::class.java) ?: return NO_DEVICE
    val driver = plugged(manager).firstOrNull {
      device.isEmpty() ||
        "%04x:%04x".format(it.device.vendorId, it.device.productId) == device
    } ?: return NO_DEVICE
    if (!manager.hasPermission(driver.device)) {
      ask(manager, driver)
      return ASKED
    }
    return try {
      val connection = manager.openDevice(driver.device) ?: return FAILED
      val port = driver.ports.first()
      port.open(connection)
      port.setParameters(baud, 8, UsbSerialPort.STOPBITS_1, UsbSerialPort.PARITY_NONE)
      // Both lines up, as a terminal leaves them: on boards whose chip
      // resets them through these, as ESP32's, neither resets it.
      port.dtr = true
      port.rts = true
      val handle = handles.incrementAndGet()
      val reader = SerialInputOutputManager(port, object : SerialInputOutputManager.Listener {
        override fun onNewData(data: ByteArray) = onUsbData(handle, data)
        override fun onRunError(error: Exception) {
          close(handle)
          onUsbClosed(handle)
        }
      })
      reader.start()
      usb[handle] = Opened(port, reader)
      handle
    } catch (error: Exception) {
      android.util.Log.w("meshctl", "cannot open the radio by USB: $error")
      FAILED
    }
  }

  /** The radios plugged in by USB, for the daemon to list, as JSON. */
  @Keep
  fun listUsb(): String {
    val manager = getSystemService(UsbManager::class.java) ?: return "[]"
    val devices = org.json.JSONArray()
    for (driver in plugged(manager)) {
      val device = driver.device
      // Its names, where Android gives them without leave.
      val maker = try { device.manufacturerName } catch (_: Exception) { null }
      val name = try { device.productName } catch (_: Exception) { null }
      devices.put(
        org.json.JSONObject()
          .put("vendor", device.vendorId)
          .put("product", device.productId)
          .put("maker", maker ?: "")
          .put("name", name ?: "")
          .put("allowed", manager.hasPermission(device))
      )
    }
    return devices.toString()
  }

  /**
   * Asks the user for leave to use a radio by USB, by its ids as
   * `vvvv:pppp`: whether meshctl has it already; if not, Android asks.
   */
  @Keep
  fun askUsb(device: String): Boolean {
    val manager = getSystemService(UsbManager::class.java) ?: return false
    val driver = plugged(manager).firstOrNull {
      "%04x:%04x".format(it.device.vendorId, it.device.productId) == device
    } ?: return false
    if (manager.hasPermission(driver.device)) {
      return true
    }
    ask(manager, driver)
    return false
  }

  private fun ask(manager: UsbManager, driver: UsbSerialDriver) {
    val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      PendingIntent.FLAG_MUTABLE
    } else {
      0
    }
    manager.requestPermission(
      driver.device,
      PendingIntent.getBroadcast(this, 0, Intent(USB_PERMISSION).setPackage(packageName), flags)
    )
  }

  /** Writes to a radio open by USB; whether it could. */
  @Keep
  fun writeUsb(handle: Int, data: ByteArray): Boolean {
    val opened = usb[handle] ?: return false
    return try {
      opened.port.write(data, WRITE_MS)
      true
    } catch (error: Exception) {
      false
    }
  }

  @Keep
  fun closeUsb(handle: Int) = close(handle)

  private fun close(handle: Int) {
    usb.remove(handle)?.let {
      it.reader.stop()
      try {
        it.port.close()
      } catch (_: Exception) {
      }
    }
  }

  private external fun runDaemon(data: String)
  private external fun stopDaemon()
  private external fun onUsbData(handle: Int, data: ByteArray)
  private external fun onUsbClosed(handle: Int)
  private external fun onBleData(handle: Int, data: ByteArray)
  private external fun onBleClosed(handle: Int)

  companion object {
    /** Posts its notification again, if it is in the foreground. */
    const val REFRESH = "me.ogarcia.meshctl.REFRESH"
    private const val CHANNEL = "listening"
    private const val NOTIFICATION = 1
    private const val STOPPING_MS = 8000L
    private const val WRITE_MS = 2000
    private const val USB_PERMISSION = "me.ogarcia.meshctl.USB_PERMISSION"
    private const val NO_DEVICE = -1
    private const val ASKED = -2
    private const val FAILED = -3
    /** What lookBle says while the user is asked to let meshctl use Bluetooth. */
    private const val ASKED_BLE = "asked"
  }
}
