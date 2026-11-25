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

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use std::ffi::{CString, c_char, c_int};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
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
struct Service {
    vm: JavaVM,
    service: GlobalRef,
}

impl meshctld::Host for Service {
    fn keep_running(&self, on: bool) {
        let told = self.vm.attach_current_thread().and_then(|mut env| {
            let called = env
                .call_method(
                    &self.service,
                    "keepListening",
                    "(Z)V",
                    &[JValue::Bool(u8::from(on))],
                )
                .map(|_| ());
            // What Java threw, to Android's log, and cleared.
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            called
        });
        if let Err(error) = told {
            tracing::warn!("cannot tell the service to stay in the foreground: {error}");
        }
    }
}

/// The running daemon's stop, once started.
static STOP: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);

/// Runs the daemon until `stopDaemon`, keeping what it knows in `data`:
/// the service's thread, which it holds all along.
#[unsafe(no_mangle)]
pub extern "system" fn Java_me_ogarcia_meshctl_ListeningService_runDaemon(
    mut env: JNIEnv,
    service: JObject,
    data: JString,
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
    let data: PathBuf = match env.get_string(&data) {
        Ok(data) => PathBuf::from(String::from(data)),
        Err(error) => {
            tracing::error!("no folder for the daemon: {error}");
            return;
        }
    };
    let host: Option<Arc<dyn meshctld::Host>> =
        match (env.get_java_vm(), env.new_global_ref(service)) {
            (Ok(vm), Ok(service)) => Some(Arc::new(Service { vm, service })),
            _ => None,
        };
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
    _env: JNIEnv,
    _service: JObject,
) {
    if let Some(stop) = STOP.lock().ok().and_then(|mut stop| stop.take()) {
        let _ = stop.send(());
    }
}
