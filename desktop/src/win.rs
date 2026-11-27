// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The app, as the daemon's host on Windows: whether closing the window
//! leaves it in the tray, listening, or quits it; and the notifications of
//! messages, as Windows' own toasts, as best it can. One for each
//! conversation, the newest in place of the one before; picked, the window
//! shows on its conversation; read anywhere, it goes from the notification
//! centre.
//!
//! Windows shows toasts only for an app it knows by its id. The portable
//! folder installs nothing, so the app tells it its id, name and icon, in
//! the user's part of the registry, as it starts. Whatever of this fails
//! is logged, and the app goes on without toasts.

use crate::notifying::{body, chat, id, notice_id, open, title};
use meshctl_api::{Notification, Peer, RadioId};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::core::{HSTRING, IInspectable};

/// The id Windows knows the app by, as Tauri's identifier.
const APP_ID: &str = "me.ogarcia.meshctl";

/// The group every toast of meshctl's goes in.
const GROUP: &str = "meshctl";

/// meshctl's icon, for Windows to show by its toasts.
const ICON: &[u8] = include_bytes!("../../assets/icons/png/128.png");

pub struct Host {
    app: AppHandle,
    /// Whether closing the window leaves the app running, as set.
    keep: Arc<AtomicBool>,
}

impl Host {
    /// The host, the app made known to Windows, its icon kept in `data`.
    pub fn new(app: AppHandle, keep: Arc<AtomicBool>, data: &Path) -> Self {
        if let Err(error) = register(data) {
            tracing::warn!("Windows will show no notifications: {error}");
        }
        Self { app, keep }
    }
}

/// Tells Windows of the app, by its id: its name, and its icon, written
/// beside what it keeps.
fn register(data: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(data)?;
    let icon = data.join("meshctl.png");
    std::fs::write(&icon, ICON)?;
    let key = windows_registry::CURRENT_USER
        .create(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
    key.set_string("DisplayName", "meshctl")?;
    key.set_string("IconUri", icon.to_string_lossy().as_ref())?;
    Ok(())
}

/// Text as XML takes it.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Shows a toast: Windows' calls on the app's main thread, where they are
/// at home.
fn show(app: &AppHandle, notification: &Notification) -> windows::core::Result<()> {
    let xml = format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        escape(&title(notification)),
        escape(&body(notification)),
    );
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&document)?;
    toast.SetTag(&HSTRING::from(notice_id(notification)))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;
    let (app, chat) = (app.clone(), chat(notification));
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
        move |_, _| {
            open(&app, &chat);
            Ok(())
        },
    ))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
}

/// Takes a conversation's toast away from the notification centre.
fn remove(tag: &str) -> windows::core::Result<()> {
    ToastNotificationManager::History()?.RemoveGroupedTagWithId(
        &HSTRING::from(tag),
        &HSTRING::from(GROUP),
        &HSTRING::from(APP_ID),
    )
}

impl meshctld::Host for Host {
    fn keep_running(&self, on: bool) {
        self.keep.store(on, Ordering::Relaxed);
    }

    fn notify(&self, notification: &Notification) {
        let (app, notification) = (self.app.clone(), notification.clone());
        let _ = self.app.run_on_main_thread(move || {
            if let Err(error) = show(&app, &notification) {
                tracing::warn!("cannot notify: {error}");
            }
        });
    }

    fn read(&self, radio: RadioId, peer: Peer) {
        let tag = id(radio, peer);
        let _ = self.app.run_on_main_thread(move || {
            let _ = remove(&tag);
        });
    }
}
