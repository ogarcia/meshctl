// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl as an app, as Rucio's: it runs the daemon within itself,
//! on this machine alone and on a port nothing else uses, and shows its
//! panel in a window of its own. The panel is the same as any browser's: it
//! talks to the daemon over HTTP and its WebSocket, not through Tauri.
//!
//! What it keeps goes next to the program on Windows, which carries it
//! along in its folder; elsewhere, in the user's data folder (inside the
//! Flatpak, under ~/.var/app/me.ogarcia.meshctl/). Closing the window keeps
//! it running, and listening to the radios: in the tray where there is one,
//! as a background app on Linux. A `meshcore://` link opened anywhere
//! opens the panel's window to add what it holds.

//! On Android the daemon runs in a service of its own, in its own process,
//! and the app's window shows its panel: see `android`.

use std::net::Ipv4Addr;
#[cfg(desktop)]
use std::net::{SocketAddr, TcpListener};
#[cfg(desktop)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Runtime, Url, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;
use tokio::sync::oneshot;
use tracing_subscriber::EnvFilter;

/// Where the daemon serves: this machine alone. It takes no token, then:
/// nothing else reaches it.
const HOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// The links it opens: those of the panel's link window that the system
/// can send it. Meshtastic's are web addresses, the browser's to open.
const SCHEMES: &[&str] = &["meshcore"];

/// The window's size and whether it is maximised, kept from one run to the
/// next. Not its place, which Wayland's compositor decides and does not let
/// a window set; nor whether it shows, which is the app's to say.
#[cfg(desktop)]
fn kept_state() -> tauri_plugin_window_state::StateFlags {
    use tauri_plugin_window_state::StateFlags;
    StateFlags::SIZE | StateFlags::MAXIMIZED
}

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod bluetooth;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod notifying;
#[cfg(target_os = "windows")]
mod win;

/// A port nothing uses now on this machine, so that the daemon does not
/// clash with a `meshctld` of its own, or anything else. Another program
/// could take it before the daemon does, in the moment between: unlikely.
#[cfg(desktop)]
fn free_port() -> u16 {
    TcpListener::bind((HOST, 0))
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map_or(7434, |address| address.port())
}

/// Where what meshctl knows is kept, on a desktop. On Android, the
/// service says: the app's own folder.
#[cfg(desktop)]
fn data_dir() -> PathBuf {
    // Beside the program, carried along with it.
    #[cfg(target_os = "windows")]
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("data")))
    {
        return dir;
    }
    dirs::data_dir().map_or_else(|| PathBuf::from("data"), |dir| dir.join("meshctl"))
}

/// The panel's address, or, for a link, the one it is shared to, as an
/// installed web app is: the panel opens it to add what it holds.
fn page(port: u16, link: Option<&Url>) -> Url {
    let base = format!("http://{HOST}:{port}/");
    match link {
        Some(link) => Url::parse_with_params(&format!("{base}share"), &[("url", link.as_str())]),
        None => Url::parse(&base),
    }
    .expect("an address on this machine")
}

/// Whether a link is one of those it opens.
fn ours(link: &Url) -> bool {
    SCHEMES.contains(&link.scheme())
}

/// Shows the window, brought to the front, if it is there: on a desktop,
/// where it may be hidden; a phone's app shows as it is opened.
fn show<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(mobile)]
    let _ = app;
    #[cfg(desktop)]
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Opens a link in the window, to add what it holds.
fn open_link<R: Runtime>(app: &AppHandle<R>, port: u16, link: &Url) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.navigate(page(port, Some(link)));
    }
    show(app);
}

/// The panel's window, on a link if given.
fn open_window<R: Runtime>(app: &AppHandle<R>, port: u16, link: Option<&Url>) {
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(page(port, link)));
    // The first time; then as it was left.
    #[cfg(desktop)]
    let window = window
        .title("meshctl")
        .inner_size(1200.0, 800.0)
        .min_inner_size(400.0, 500.0);
    if let Err(error) = window.build() {
        tracing::error!("cannot open the window: {error}");
    }
}

/// Waits until the daemon serves, so that the window does not open on a
/// connection refused; a minute at most, and then it opens anyway.
async fn serving(port: u16) {
    for _ in 0..600 {
        if tokio::net::TcpStream::connect((HOST, port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs the app: the daemon, and the window over it.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMA-BUF renderer breaks on some Wayland compositors and
    // GPUs; without it, it draws on a path that works. Set before GTK
    // starts, unless the user set it.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: the first thing in main, before any other thread runs.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    // D-Bus's library, under the portal's, warns of what is not wrong: that
    // it read a request's properties before the portal made it. Its errors
    // only, unless RUST_LOG says otherwise.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,zbus=error".into()),
        )
        .init();

    // On a desktop, a port free now; on Android, the daemon's own, which
    // its service, in a process of its own, serves on.
    #[cfg(desktop)]
    let port = free_port();
    #[cfg(target_os = "android")]
    let port = android::port();
    // On a desktop, the daemon, started once where it keeps what it knows
    // is known, and stopped when the app quits, so that it lets the radios
    // go and closes what it keeps cleanly. On Android, the service runs it,
    // started as the window opens.
    let stop = Arc::new(Mutex::new(None::<oneshot::Sender<()>>));
    let daemon = Arc::new(Mutex::new(None::<tauri::async_runtime::JoinHandle<()>>));
    // Whether closing the window leaves it running, listening, as the
    // daemon says it was set: on Linux and Windows, where it lets it be
    // set; always elsewhere.
    #[cfg(desktop)]
    let keep = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let builder = tauri::Builder::default();
    // The first: launched again, the one running shows itself, and takes
    // the link it was launched with, instead of a second daemon starting
    // over the same data.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(kept_state())
                .build(),
        )
        // Closing the window hides it: the app, and the daemon, go on; or,
        // set not to, it quits.
        .on_window_event({
            let keep = Arc::clone(&keep);
            move |window, event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // Its size kept now, while it is there: hidden, the app
                    // may be ended later without a chance to.
                    use tauri_plugin_window_state::AppHandleExt;
                    let _ = window.app_handle().save_window_state(kept_state());
                    api.prevent_close();
                    if keep.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = window.hide();
                    } else {
                        window.app_handle().exit(0);
                    }
                }
            }
        });
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_deep_link::init());

    let (started_stop, started_daemon) = (Arc::clone(&stop), Arc::clone(&daemon));
    let app = builder
        .setup(move |app| {
            #[cfg(desktop)]
            {
                // On Linux and Windows, the app as its host: whether it goes
                // on with its window closed, and the notifications.
                #[cfg(target_os = "linux")]
                let host: Option<Arc<dyn meshctld::Host>> = Some(Arc::new(linux::Shared(
                    linux::Host::new(app.handle().clone(), Arc::clone(&keep)),
                )));
                #[cfg(target_os = "windows")]
                let host: Option<Arc<dyn meshctld::Host>> = Some(Arc::new(win::Host::new(
                    app.handle().clone(),
                    Arc::clone(&keep),
                    &data_dir(),
                )));
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let host = None;
                // Radios by Bluetooth, reached by the app for the daemon.
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                meshctl_core::link::ble::set_opener(Arc::new(bluetooth::Desktop));
                let config = meshctld::Config {
                    data: data_dir(),
                    listen: SocketAddr::from((HOST, port)),
                    token: None,
                    host,
                };
                let (stopping, stopped) = oneshot::channel::<()>();
                let task = tauri::async_runtime::spawn(async move {
                    let until = async move {
                        let _ = stopped.await;
                    };
                    if let Err(error) = meshctld::run_until(config, until).await {
                        tracing::error!("the daemon stopped: {error:#}");
                    }
                });
                if let Ok(mut stop) = started_stop.lock() {
                    *stop = Some(stopping);
                }
                if let Ok(mut daemon) = started_daemon.lock() {
                    *daemon = Some(task);
                }
            }
            #[cfg(mobile)]
            let _ = (&started_stop, &started_daemon);

            #[cfg(any(target_os = "windows", target_os = "macos"))]
            tray(app)?;
            #[cfg(target_os = "linux")]
            {
                tauri::async_runtime::spawn(background());
                quit_on_signal(app.handle().clone());
            }
            // Where it is not installed, as the portable folder, it tells
            // the system the links it opens itself: the Flatpak's says so
            // in its own desktop entry.
            #[cfg(target_os = "windows")]
            if let Err(error) = app.deep_link().register_all() {
                tracing::warn!("cannot take meshcore:// links: {error}");
            }

            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                if let Some(link) = event.urls().into_iter().find(ours) {
                    open_link(&handle, port, &link);
                }
            });
            // Launched with a link: the window opens on it.
            let first = app
                .deep_link()
                .get_current()
                .ok()
                .flatten()
                .and_then(|links| links.into_iter().find(ours));

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                serving(port).await;
                open_window(&handle, port, first.as_ref());
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("meshctl's app cannot start");

    app.run(move |_app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            // The daemon stopped, and waited for, a few seconds at most:
            // stuck, it does not keep the app from ending.
            if let Some(stop) = stop.lock().ok().and_then(|mut stop| stop.take()) {
                let _ = stop.send(());
            }
            if let Some(task) = daemon.lock().ok().and_then(|mut daemon| daemon.take()) {
                let _ = tauri::async_runtime::block_on(async {
                    tokio::time::timeout(Duration::from_secs(8), task).await
                });
            }
        }
    });
}

/// The tray, where there is one: a click shows the window; its menu shows it
/// or quits, which stops the daemon.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let spanish = sys_locale::get_locale().is_some_and(|locale| locale.starts_with("es"));
    let (show_words, quit_words) = if spanish {
        ("Mostrar meshctl", "Salir")
    } else {
        ("Show meshctl", "Quit")
    };
    let show_item = MenuItem::with_id(app, "show", show_words, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", quit_words, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("meshctl")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Asks the desktop, through the XDG portal, to let it go on with its
/// window closed, as a background app it lists and can stop. Outside a
/// portal it runs as any other program, its window hidden all the same.
#[cfg(target_os = "linux")]
async fn background() {
    use ashpd::desktop::background::Background;
    if let Err(error) = Background::request()
        .reason("meshctl goes on listening to your radios while its window is closed")
        .auto_start(false)
        .dbus_activatable(false)
        .send()
        .await
        .and_then(|request| request.response())
    {
        tracing::info!("running without the background portal: {error}");
    }
}

/// Quits on SIGTERM or Ctrl+C, the daemon stopped cleanly: the desktop's
/// own stop for background apps is a SIGKILL, which nothing can catch.
#[cfg(target_os = "linux")]
fn quit_on_signal(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut terminate), Ok(mut interrupt)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
        app.exit(0);
    });
}
