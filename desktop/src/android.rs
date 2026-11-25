// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The daemon on Android: in a process of its own, the service's, apart
//! from the app's window, so that the window can go and come back as
//! Android has it while the daemon goes on listening. The service starts
//! it through JNI, and the daemon tells the service, through JNI, whether
//! to stay in the foreground, as it was set to.
//!
//! Both processes are the app's, of the same user id, and the daemon
//! serves on a port taken from it: each finds it without being told.
//!
//! Radios plugged in by USB, which Android gives no serial device for,
//! the service opens through Android's own USB: their bytes come in
//! through `onUsbData` and go out through its `writeUsb`, as a stream the
//! daemon takes as it would a serial port's. Radios by Bluetooth, the
//! service reaches too, their payloads coming through `onBleData` and
//! going through its `writeBle`, one by one.

use jni::errors::LogErrorAndDefault;
use jni::objects::{JByteArray, JObject, JString};
use jni::refs::Global;
use jni::sys::jint;
use jni::{Env, EnvUnowned, JValue, JavaVM, jni_sig, jni_str};
use meshctl_core::link::Framing;
use meshctl_core::link::ble::{BleOpener, Looked, Nearby, Pipe};
use meshctl_core::link::usb::{Opening, UsbDevice, UsbOpener, UsbStream};
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

unsafe extern "C" {
    fn getuid() -> u32;
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// The port the daemon serves on: one of its own for the app, taken from
/// its user id, which both its processes share. Clear of the daemon's
/// usual 7434, which a `meshctld` of the user's may hold.
pub fn port() -> u16 {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { getuid() };
    30_000 + u16::try_from(uid % 10_000).unwrap_or(0)
}

/// A line of the daemon's logs at a time, to Android's log, as Tauri does
/// for the window's process.
struct Logcat(Vec<u8>);

impl Write for Logcat {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(bytes);
        while let Some(end) = self.0.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.0.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).replace('\0', "");
            if let (Ok(tag), Ok(text)) = (CString::new("meshctld"), CString::new(text)) {
                // SAFETY: two valid C strings; 4 is Android's INFO.
                unsafe { __android_log_write(4, tag.as_ptr(), text.as_ptr()) };
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The service, as the daemon's host: told whether to stay in the
/// foreground, it is called on, in Java.
struct Service(Global<JObject<'static>>);

impl Service {
    /// Calls on the service, from any thread, the thread attached for good
    /// and in a frame of its own each time, so that a thread that calls
    /// again and again keeps no references; what Java throws comes back as
    /// an error, to the log.
    fn call<T>(
        &self,
        method: &str,
        call: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>,
    ) -> Option<T> {
        let called =
            JavaVM::singleton().and_then(|vm| vm.attach_current_thread(|env| call(env, &self.0)));
        match called {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!("cannot call the service's {method}: {error}");
                None
            }
        }
    }
}

impl meshctld::Host for Service {
    fn keep_running(&self, on: bool) {
        self.call("keepListening", |env, service| {
            env.call_method(
                service,
                jni_str!("keepListening"),
                jni_sig!("(Z)V"),
                &[JValue::Bool(on)],
            )
            .map(|_| ())
        });
    }

    fn notify(&self, notification: &meshctl_api::Notification) {
        let Ok(json) = serde_json::to_string(notification) else {
            return;
        };
        self.call("notifyMessage", |env, service| {
            let json = env.new_string(json)?;
            env.call_method(
                service,
                jni_str!("notifyMessage"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&json)],
            )
            .map(|_| ())
        });
    }

    fn read(&self, radio: meshctl_api::RadioId, peer: meshctl_api::Peer) {
        let (Ok(peer), Ok(radio)) = (serde_json::to_string(&peer), jint::try_from(radio)) else {
            return;
        };
        self.call("readConversation", |env, service| {
            let peer = env.new_string(peer)?;
            env.call_method(
                service,
                jni_str!("readConversation"),
                jni_sig!("(ILjava/lang/String;)V"),
                &[JValue::Int(radio), JValue::Object(&peer)],
            )
            .map(|_| ())
        });
    }
}

/// Each radio open by USB, by the service's handle for it: where its bytes
/// go as they come, and none as it closes.
type UsbRadios = HashMap<jint, mpsc::UnboundedSender<Option<Vec<u8>>>>;
static USB: Mutex<Option<UsbRadios>> = Mutex::new(None);

/// Opens radios by USB through the service.
struct UsbBridge(Arc<Service>);

impl UsbOpener for UsbBridge {
    fn open(&self, device: Option<String>, baud: u32) -> Opening {
        let service = Arc::clone(&self.0);
        Box::pin(async move {
            let opened = {
                let service = Arc::clone(&service);
                tokio::task::spawn_blocking(move || {
                    service.call("openUsb", |env, service| {
                        let device = env.new_string(device.unwrap_or_default())?;
                        env.call_method(
                            service,
                            jni_str!("openUsb"),
                            jni_sig!("(Ljava/lang/String;I)I"),
                            &[JValue::Object(&device), JValue::Int(baud as jint)],
                        )?
                        .i()
                    })
                })
                .await
                .ok()
                .flatten()
            };
            let handle = match opened {
                Some(handle) if handle >= 0 => handle,
                Some(NO_DEVICE) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no radio plugged in by USB",
                    ));
                }
                Some(ASKED) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "waiting for leave to use the radio by USB",
                    ));
                }
                _ => {
                    return Err(std::io::Error::other("the radio by USB would not open"));
                }
            };
            Ok(Box::new(bridge(service, handle)) as Box<dyn UsbStream>)
        })
    }

    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<UsbDevice>> + Send>> {
        let bridge = UsbBridge(Arc::clone(&self.0));
        Box::pin(async move {
            tokio::task::spawn_blocking(move || bridge.devices())
                .await
                .unwrap_or_default()
        })
    }

    fn ask(
        &self,
        device: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> {
        let service = Arc::clone(&self.0);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                service.call("askUsb", |env, service| {
                    let device = env.new_string(device)?;
                    env.call_method(
                        service,
                        jni_str!("askUsb"),
                        jni_sig!("(Ljava/lang/String;)Z"),
                        &[JValue::Object(&device)],
                    )?
                    .z()
                })
            })
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
        })
    }
}

impl UsbBridge {
    /// The radios by USB, as the service lists them, in JSON.
    fn devices(&self) -> Vec<UsbDevice> {
        let listed = self.0.call("listUsb", |env, service| {
            let listed = env
                .call_method(
                    service,
                    jni_str!("listUsb"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            env.cast_local::<JString>(listed)?.try_to_string(env)
        });
        let Some(listed) = listed else {
            return Vec::new();
        };
        let Ok(serde_json::Value::Array(devices)) = serde_json::from_str(&listed) else {
            return Vec::new();
        };
        devices
            .iter()
            .filter_map(|device| {
                let text = |key: &str| {
                    device[key]
                        .as_str()
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty())
                };
                Some(UsbDevice {
                    vendor: u16::try_from(device["vendor"].as_u64()?).ok()?,
                    product_id: u16::try_from(device["product"].as_u64()?).ok()?,
                    maker: text("maker"),
                    product: text("name"),
                    allowed: device["allowed"].as_bool().unwrap_or(false),
                })
            })
            .collect()
    }
}

/// What `openUsb` answers when there is no such device, and when it asked
/// for leave to use it, which the user is yet to give.
const NO_DEVICE: jint = -1;
const ASKED: jint = -2;

/// A radio's stream, its bytes carried to and from the service's handle
/// for it: what comes in, through the registry; what goes out, by a
/// thread of its own, as writing waits on the device.
fn bridge(service: Arc<Service>, handle: jint) -> tokio::io::DuplexStream {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (incoming, mut arrived) = mpsc::unbounded_channel();
    if let Ok(mut usb) = USB.lock() {
        usb.get_or_insert_with(HashMap::new)
            .insert(handle, incoming);
    }
    let (outgoing, to_write) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = Arc::clone(&service);
    std::thread::spawn(move || {
        while let Ok(bytes) = to_write.recv() {
            let written = writer.call("writeUsb", |env, service| {
                let array = env.byte_array_from_slice(&bytes)?;
                env.call_method(
                    service,
                    jni_str!("writeUsb"),
                    jni_sig!("(I[B)Z"),
                    &[JValue::Int(handle), JValue::Object(&array)],
                )?
                .z()
            });
            if written != Some(true) {
                break;
            }
        }
        writer.call("closeUsb", |env, service| {
            env.call_method(
                service,
                jni_str!("closeUsb"),
                jni_sig!("(I)V"),
                &[JValue::Int(handle)],
            )
            .map(|_| ())
        });
        if let Ok(mut usb) = USB.lock()
            && let Some(usb) = usb.as_mut()
        {
            usb.remove(&handle);
        }
    });
    tokio::spawn(async move {
        let (mut from_link, mut to_link) = tokio::io::split(theirs);
        let read = async {
            let mut buffer = vec![0u8; 4096];
            loop {
                match from_link.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if outgoing.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        };
        let write = async {
            while let Some(Some(bytes)) = arrived.recv().await {
                if to_link.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        };
        // Either side done ends both: the link drops its end, or the device
        // is gone; the writing thread then closes the device.
        tokio::select! {
            () = read => {}
            () = write => {}
        }
        drop(outgoing);
    });
    ours
}

/// Bytes from a radio by USB, from the service's thread that reads it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_onUsbData<'local>(
    mut env: EnvUnowned<'local>,
    _service: JObject<'local>,
    handle: jint,
    data: JByteArray<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let bytes = env.convert_byte_array(&data)?;
        if let Ok(usb) = USB.lock()
            && let Some(sender) = usb.as_ref().and_then(|usb| usb.get(&handle))
        {
            let _ = sender.send(Some(bytes));
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// A radio by USB gone: unplugged, or failing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_onUsbClosed(
    _env: EnvUnowned,
    _service: JObject,
    handle: jint,
) {
    if let Ok(mut usb) = USB.lock()
        && let Some(sender) = usb.as_mut().and_then(|usb| usb.remove(&handle))
    {
        let _ = sender.send(None);
    }
}

/// Each radio reached by Bluetooth, by the service's handle for it: where
/// its payloads go as they come.
static BLE: Mutex<Option<HashMap<jint, mpsc::UnboundedSender<Vec<u8>>>>> = Mutex::new(None);

/// Reaches radios by Bluetooth through the service.
struct BleBridge(Arc<Service>);

impl BleOpener for BleBridge {
    fn open(
        &self,
        address: String,
        framing: Framing,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<Pipe>> + Send>> {
        let service = Arc::clone(&self.0);
        Box::pin(async move {
            let opened = {
                let service = Arc::clone(&service);
                tokio::task::spawn_blocking(move || {
                    service.call("openBle", |env, service| {
                        let address = env.new_string(address)?;
                        env.call_method(
                            service,
                            jni_str!("openBle"),
                            jni_sig!("(Ljava/lang/String;Z)I"),
                            &[
                                JValue::Object(&address),
                                JValue::Bool(framing == Framing::Meshcore),
                            ],
                        )?
                        .i()
                    })
                })
                .await
                .ok()
                .flatten()
            };
            let handle = match opened {
                Some(handle) if handle >= 0 => handle,
                Some(-2) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "waiting for leave to use Bluetooth",
                    ));
                }
                Some(-4) => return Err(std::io::Error::other("Bluetooth is off")),
                Some(-5) => return Err(std::io::Error::other("the radio was not paired")),
                _ => {
                    return Err(std::io::Error::other(
                        "the radio by Bluetooth did not answer",
                    ));
                }
            };
            let (to_radio, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
            let (incoming, from_radio) = mpsc::unbounded_channel::<Vec<u8>>();
            if let Ok(mut ble) = BLE.lock() {
                ble.get_or_insert_with(HashMap::new)
                    .insert(handle, incoming);
            }
            // Each payload to the radio, by a thread of its own, as writing
            // waits on it; the link gone, the radio let go.
            std::thread::spawn(move || {
                while let Some(payload) = outgoing.blocking_recv() {
                    let written = service.call("writeBle", |env, service| {
                        let array = env.byte_array_from_slice(&payload)?;
                        env.call_method(
                            service,
                            jni_str!("writeBle"),
                            jni_sig!("(I[B)Z"),
                            &[JValue::Int(handle), JValue::Object(&array)],
                        )?
                        .z()
                    });
                    if written != Some(true) {
                        break;
                    }
                }
                service.call("closeBle", |env, service| {
                    env.call_method(
                        service,
                        jni_str!("closeBle"),
                        jni_sig!("(I)V"),
                        &[JValue::Int(handle)],
                    )
                    .map(|_| ())
                });
                if let Ok(mut ble) = BLE.lock()
                    && let Some(ble) = ble.as_mut()
                {
                    ble.remove(&handle);
                }
            });
            Ok((to_radio, from_radio))
        })
    }

    fn look(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Looked> + Send>> {
        let service = Arc::clone(&self.0);
        Box::pin(async move {
            let looked = tokio::task::spawn_blocking(move || {
                service.call("lookBle", |env, service| {
                    let looked = env
                        .call_method(
                            service,
                            jni_str!("lookBle"),
                            jni_sig!("()Ljava/lang/String;"),
                            &[],
                        )?
                        .l()?;
                    env.cast_local::<JString>(looked)?.try_to_string(env)
                })
            })
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
            if looked == "asked" {
                return Looked::Asked;
            }
            let Ok(serde_json::Value::Array(heard)) = serde_json::from_str(&looked) else {
                return Looked::Heard(Vec::new());
            };
            Looked::Heard(
                heard
                    .iter()
                    .filter_map(|radio| {
                        Some(Nearby {
                            address: radio["address"].as_str()?.to_owned(),
                            name: radio["name"]
                                .as_str()
                                .filter(|n| !n.is_empty())
                                .map(str::to_owned),
                            rssi: radio["rssi"].as_i64().and_then(|r| i16::try_from(r).ok()),
                            framing: match radio["kind"].as_str() {
                                Some("meshtastic") => Some(Framing::Meshtastic),
                                Some("meshcore") => Some(Framing::Meshcore),
                                _ => None,
                            },
                        })
                    })
                    .collect(),
            )
        })
    }
}

/// A payload from a radio by Bluetooth, from the service's callbacks.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_onBleData<'local>(
    mut env: EnvUnowned<'local>,
    _service: JObject<'local>,
    handle: jint,
    data: JByteArray<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let payload = env.convert_byte_array(&data)?;
        if let Ok(ble) = BLE.lock()
            && let Some(sender) = ble.as_ref().and_then(|ble| ble.get(&handle))
        {
            let _ = sender.send(payload);
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// A radio by Bluetooth gone: out of reach, or failing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_onBleClosed(
    _env: EnvUnowned,
    _service: JObject,
    handle: jint,
) {
    if let Ok(mut ble) = BLE.lock()
        && let Some(ble) = ble.as_mut()
    {
        // Its sender gone, the link sees it go.
        ble.remove(&handle);
    }
}

/// The running daemon's stop, once started.
static STOP: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);

/// Runs the daemon until `stopDaemon`, keeping what it knows in `data`:
/// the service's thread, which it holds all along.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_runDaemon<'local>(
    mut env: EnvUnowned<'local>,
    service: JObject<'local>,
    data: JString<'local>,
) {
    static LOGS: OnceLock<()> = OnceLock::new();
    LOGS.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter("info")
            .with_ansi(false)
            .without_time()
            .with_writer(|| Logcat(Vec::new()))
            .init();
    });
    // Its folder and the service, kept to call on from other threads.
    let taken = env
        .with_env(|env| -> jni::errors::Result<_> {
            let data = PathBuf::from(data.try_to_string(env)?);
            let service = env.new_global_ref(&service).ok().map(Service);
            Ok(Some((data, service)))
        })
        .resolve::<LogErrorAndDefault>();
    let Some((data, service)) = taken else {
        tracing::error!("no folder for the daemon");
        return;
    };
    let service = service.map(Arc::new);
    if let Some(service) = &service {
        meshctl_core::link::usb::set_opener(Arc::new(UsbBridge(Arc::clone(service))));
        meshctl_core::link::ble::set_opener(Arc::new(BleBridge(Arc::clone(service))));
    }
    let host = service.map(|service| service as Arc<dyn meshctld::Host>);
    let config = meshctld::Config {
        data,
        listen: SocketAddr::from((Ipv4Addr::LOCALHOST, port())),
        token: None,
        host,
    };
    let (stop, stopped) = oneshot::channel::<()>();
    if let Ok(mut kept) = STOP.lock() {
        *kept = Some(stop);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("cannot start the daemon's runtime: {error}");
            return;
        }
    };
    runtime.block_on(async move {
        let until = async move {
            let _ = stopped.await;
        };
        if let Err(error) = meshctld::run_until(config, until).await {
            tracing::error!("the daemon stopped: {error:#}");
        }
    });
}

/// Stops the daemon, which lets the radios go and closes what it keeps.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_stopDaemon(
    _env: EnvUnowned,
    _service: JObject,
) {
    if let Some(stop) = STOP.lock().ok().and_then(|mut stop| stop.take()) {
        let _ = stop.send(());
    }
}
