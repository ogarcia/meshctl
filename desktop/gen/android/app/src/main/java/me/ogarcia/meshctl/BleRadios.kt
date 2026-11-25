package me.ogarcia.meshctl

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONArray
import org.json.JSONObject

/**
 * Radios by Bluetooth, for the daemon: those nearby, looked for, and each
 * reached, its payloads to and from it as its firmware's service carries
 * them. Meshtastic's: written to ToRadio, read from FromRadio until it is
 * empty, as FromNum says there is more. MeshCore's, over Nordic's UART:
 * written to RX, and each notification of TX one.
 *
 * Android does one thing at a time with a device over GATT, each answered
 * by a callback: they wait in a queue, the next as the last is answered.
 */
@SuppressLint("MissingPermission")
class BleRadios(
  private val context: Context,
  private val onData: (Int, ByteArray) -> Unit,
  private val onClosed: (Int) -> Unit,
) {
  private val manager = context.getSystemService(BluetoothManager::class.java)
  private val links = ConcurrentHashMap<Int, Link>()
  private val handles = AtomicInteger(0)

  /** Whether the user lets meshctl use Bluetooth: if not, the app asks. */
  fun allowed(): Boolean {
    val wanted = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      listOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else {
      listOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }
    if (wanted.all {
        ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
      }) {
      return true
    }
    context.startActivity(
      Intent(context, MainActivity::class.java)
        .setAction(MainActivity.ASK_BLUETOOTH)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    )
    return false
  }

  /**
   * The radios heard nearby in a few seconds, as JSON: by the service they
   * say they have, or their names; none with Bluetooth off. Null while the
   * user is asked to let meshctl use it.
   */
  fun look(): String? {
    if (!allowed()) {
      return null
    }
    val scanner = manager?.adapter?.takeIf { it.isEnabled }?.bluetoothLeScanner ?: return "[]"
    val heard = ConcurrentHashMap<String, JSONObject>()
    val callback = object : ScanCallback() {
      override fun onScanResult(type: Int, result: ScanResult) {
        val services = result.scanRecord?.serviceUuids?.map { it.uuid } ?: emptyList()
        val name = result.scanRecord?.deviceName ?: result.device.name
        val kind = when {
          MESHTASTIC in services -> "meshtastic"
          UART in services -> "meshcore"
          name?.startsWith("Meshtastic") == true -> "meshtastic"
          name?.startsWith("MeshCore") == true -> "meshcore"
          else -> return
        }
        heard[result.device.address] = JSONObject()
          .put("address", result.device.address)
          .put("name", name ?: "")
          .put("rssi", result.rssi)
          .put("kind", kind)
      }
    }
    val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build()
    scanner.startScan(null, settings, callback)
    Thread.sleep(LOOKING_MS)
    scanner.stopScan(callback)
    return JSONArray(heard.values.sortedByDescending { it.optInt("rssi") }).toString()
  }

  /**
   * Reaches a radio, by its address, for its firmware: paired first if it
   * is not, Android asking the user for its PIN. Its handle once ready, or
   * a negative number if not.
   */
  fun open(address: String, meshcore: Boolean): Int {
    if (!allowed()) {
      return ASKED
    }
    val adapter = manager?.adapter?.takeIf { it.isEnabled } ?: return OFF
    val device = try {
      adapter.getRemoteDevice(address)
    } catch (_: IllegalArgumentException) {
      return FAILED
    }
    if (device.bondState != BluetoothDevice.BOND_BONDED) {
      device.createBond()
      val until = System.currentTimeMillis() + PAIRING_MS
      while (device.bondState != BluetoothDevice.BOND_BONDED &&
        System.currentTimeMillis() < until
      ) {
        Thread.sleep(POLL_MS)
      }
      if (device.bondState != BluetoothDevice.BOND_BONDED) {
        return UNPAIRED
      }
    }
    val handle = handles.incrementAndGet()
    val link = Link(handle, meshcore)
    links[handle] = link
    link.gatt = device.connectGatt(context, false, link, BluetoothDevice.TRANSPORT_LE)
    if (!link.ready.await(CONNECTING_MS, TimeUnit.MILLISECONDS) || !link.up) {
      close(handle)
      return FAILED
    }
    return handle
  }

  /** Sends a payload to a radio reached; whether it is there to take it. */
  fun write(handle: Int, data: ByteArray): Boolean {
    val link = links[handle] ?: return false
    link.write(data)
    return true
  }

  fun close(handle: Int) {
    links.remove(handle)?.let {
      it.gatt?.disconnect()
      it.gatt?.close()
    }
  }

  fun closeAll() = links.keys.forEach { close(it) }

  /** A radio reached, and the queue of what is done with it. */
  private inner class Link(val handle: Int, val meshcore: Boolean) : BluetoothGattCallback() {
    var gatt: BluetoothGatt? = null
    val ready = CountDownLatch(1)

    @Volatile
    var up = false
    private var toRadio: BluetoothGattCharacteristic? = null
    private var fromRadio: BluetoothGattCharacteristic? = null
    private val queue = ArrayDeque<() -> Boolean>()
    private var busy = false

    /** Something to do, once what is under way is answered. */
    private fun enqueue(op: () -> Boolean) {
      synchronized(queue) {
        queue.addLast(op)
        if (busy) {
          return
        }
      }
      next()
    }

    /** The next thing to do, if any: one that cannot start is skipped. */
    private fun next() {
      while (true) {
        val op = synchronized(queue) {
          val op = queue.removeFirstOrNull()
          busy = op != null
          op
        } ?: return
        if (op()) {
          return
        }
      }
    }

    private fun answered() {
      synchronized(queue) { busy = false }
      next()
    }

    fun write(data: ByteArray) {
      enqueue {
        val gatt = gatt
        val to = toRadio
        if (gatt == null || to == null) {
          false
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
          gatt.writeCharacteristic(
            to, data, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
          ) == BluetoothStatusCodes.SUCCESS
        } else {
          @Suppress("DEPRECATION")
          to.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
          @Suppress("DEPRECATION")
          to.value = data
          @Suppress("DEPRECATION")
          gatt.writeCharacteristic(to)
        }
      }
    }

    /** Meshtastic's: what the radio has for us, read until there is none. */
    private fun drain() {
      enqueue {
        val gatt = gatt
        val from = fromRadio
        gatt != null && from != null && gatt.readCharacteristic(from)
      }
    }

    private fun notify(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
      enqueue {
        gatt.setCharacteristicNotification(characteristic, true)
        val descriptor = characteristic.getDescriptor(NOTIFICATIONS) ?: return@enqueue false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
          gatt.writeDescriptor(
            descriptor, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
          ) == BluetoothStatusCodes.SUCCESS
        } else {
          @Suppress("DEPRECATION")
          descriptor.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
          @Suppress("DEPRECATION")
          gatt.writeDescriptor(descriptor)
        }
      }
    }

    override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, state: Int) {
      when (state) {
        BluetoothProfile.STATE_CONNECTED -> gatt.requestMtu(MTU)
        BluetoothProfile.STATE_DISCONNECTED -> {
          ready.countDown()
          if (links.remove(handle) != null) {
            gatt.close()
            onClosed(handle)
          }
        }
      }
    }

    override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
      gatt.discoverServices()
    }

    override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
      if (meshcore) {
        val service = gatt.getService(UART)
        toRadio = service?.getCharacteristic(UART_RX)
        val tx = service?.getCharacteristic(UART_TX)
        if (toRadio == null || tx == null) {
          ready.countDown()
          return
        }
        notify(gatt, tx)
      } else {
        val service = gatt.getService(MESHTASTIC)
        toRadio = service?.getCharacteristic(TO_RADIO)
        fromRadio = service?.getCharacteristic(FROM_RADIO)
        val fromNum = service?.getCharacteristic(FROM_NUM)
        if (toRadio == null || fromRadio == null || fromNum == null) {
          ready.countDown()
          return
        }
        notify(gatt, fromNum)
      }
      // Once notifications are on, it is ready.
      enqueue {
        up = true
        ready.countDown()
        false
      }
    }

    override fun onDescriptorWrite(gatt: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) =
      answered()

    override fun onCharacteristicWrite(
      gatt: BluetoothGatt,
      characteristic: BluetoothGattCharacteristic,
      status: Int
    ) {
      if (!meshcore) {
        drain()
      }
      answered()
    }

    override fun onCharacteristicRead(
      gatt: BluetoothGatt,
      characteristic: BluetoothGattCharacteristic,
      value: ByteArray,
      status: Int
    ) = read(value, status)

    @Deprecated("Before Android 13")
    override fun onCharacteristicRead(
      gatt: BluetoothGatt,
      characteristic: BluetoothGattCharacteristic,
      status: Int
    ) {
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
        @Suppress("DEPRECATION")
        read(characteristic.value ?: ByteArray(0), status)
      }
    }

    private fun read(value: ByteArray, status: Int) {
      if (status == BluetoothGatt.GATT_SUCCESS && value.isNotEmpty()) {
        onData(handle, value)
        drain()
      }
      answered()
    }

    override fun onCharacteristicChanged(
      gatt: BluetoothGatt,
      characteristic: BluetoothGattCharacteristic,
      value: ByteArray
    ) = changed(characteristic, value)

    @Deprecated("Before Android 13")
    override fun onCharacteristicChanged(
      gatt: BluetoothGatt,
      characteristic: BluetoothGattCharacteristic
    ) {
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
        @Suppress("DEPRECATION")
        changed(characteristic, characteristic.value ?: ByteArray(0))
      }
    }

    private fun changed(characteristic: BluetoothGattCharacteristic, value: ByteArray) {
      if (meshcore) {
        if (characteristic.uuid == UART_TX && value.isNotEmpty()) {
          onData(handle, value)
        }
      } else if (characteristic.uuid == FROM_NUM) {
        drain()
      }
    }
  }

  companion object {
    private val MESHTASTIC = UUID.fromString("6ba1b218-15a8-461f-9fa8-5dcae273eafd")
    private val TO_RADIO = UUID.fromString("f75c76d2-129e-4dad-a1dd-7866124401e7")
    private val FROM_RADIO = UUID.fromString("2c55e69e-4993-11ed-b878-0242ac120002")
    private val FROM_NUM = UUID.fromString("ed9da18c-a800-4f66-a670-aa7547e34453")
    private val UART = UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e")
    private val UART_RX = UUID.fromString("6e400002-b5a3-f393-e0a9-e50e24dcca9e")
    private val UART_TX = UUID.fromString("6e400003-b5a3-f393-e0a9-e50e24dcca9e")
    private val NOTIFICATIONS = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
    private const val MTU = 512
    private const val LOOKING_MS = 4000L
    private const val PAIRING_MS = 60_000L
    private const val CONNECTING_MS = 20_000L
    private const val POLL_MS = 500L
    const val ASKED = -2
    const val FAILED = -3
    const val OFF = -4
    const val UNPAIRED = -5
  }
}
