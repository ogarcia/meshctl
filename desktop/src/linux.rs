// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The app, as the daemon's host on Linux's desktops: whether closing the
//! window leaves it listening, or quits it; and the notifications of
//! messages, through the XDG portal, as a Flatpak shows them. One for each
//! conversation, the newest in place of the one before; picked, the window
//! shows on its conversation; read anywhere, it goes.

use futures_util::StreamExt;
use meshctl_api::{Notification, Peer, RadioId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tokio::sync::OnceCell;

use ashpd::desktop::notification::{Category, Notification as Shown, NotificationProxy, Priority};

/// What opens a notification picked: the default, the only one.
const OPEN: &str = "open";

pub struct Host {
    app: AppHandle,
    /// Whether closing the window leaves the app running, as set.
    keep: Arc<AtomicBool>,
    /// The portal, reached once, when first needed.
    portal: OnceCell<Option<NotificationProxy>>,
    /// Each notification shown, by its id: its conversation, as the panel
    /// takes it, in JSON.
    shown: Mutex<HashMap<String, String>>,
    /// The panel's words for a message whose text is not shown.
    new_message: &'static str,
}

impl Host {
    pub fn new(app: AppHandle, keep: Arc<AtomicBool>) -> Arc<Self> {
        let spanish = sys_locale::get_locale().is_some_and(|locale| locale.starts_with("es"));
        Arc::new(Self {
            app,
            keep,
            portal: OnceCell::new(),
            shown: Mutex::new(HashMap::new()),
            new_message: if spanish {
                "Mensaje nuevo"
            } else {
                "New message"
            },
        })
    }

    /// The portal; none outside one, and then no notifications. Reached, a
    /// notification picked is followed from then on.
    async fn portal(self: &Arc<Self>) -> Option<&NotificationProxy> {
        self.portal
            .get_or_init(|| async {
                match NotificationProxy::new().await {
                    Ok(proxy) => {
                        self.follow();
                        Some(proxy)
                    }
                    Err(error) => {
                        tracing::info!("no notifications without their portal: {error}");
                        None
                    }
                }
            })
            .await
            .as_ref()
    }

    /// Opens the conversation of each notification picked, from then on:
    /// the window shown, on it. Through a portal's proxy of its own, which
    /// what it hears borrows for as long as it listens.
    fn follow(self: &Arc<Self>) {
        let host = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let proxy = match NotificationProxy::new().await {
                Ok(proxy) => proxy,
                Err(error) => {
                    tracing::warn!("cannot follow the notifications picked: {error}");
                    return;
                }
            };
            let actions = match proxy.receive_action_invoked().await {
                Ok(actions) => actions,
                Err(error) => {
                    tracing::warn!("cannot follow the notifications picked: {error}");
                    return;
                }
            };
            let mut actions = std::pin::pin!(actions);
            while let Some(action) = actions.next().await {
                let chat = host
                    .shown
                    .lock()
                    .ok()
                    .and_then(|mut shown| shown.remove(action.id()));
                if let Some(chat) = chat {
                    host.open(&chat);
                }
            }
        });
    }

    /// Shows the window, and tells its panel to open a conversation.
    fn open(&self, chat: &str) {
        crate::show(&self.app);
        let Some(window) = self.app.get_webview_window("main") else {
            return;
        };
        let detail = serde_json::to_string(chat).unwrap_or_default();
        let _ = window.eval(format!(
            "window.dispatchEvent(new CustomEvent('meshctl-chat', {{ detail: {detail} }}))"
        ));
    }
}

/// A conversation's notification's id: its radio, and its channel or node.
fn id(radio: RadioId, peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => format!("meshctl-{radio}-c{channel}"),
        Peer::Node { node } => format!("meshctl-{radio}-n{node}"),
    }
}

/// The host's calls come from the daemon's tasks, and wait for nothing:
/// what they ask of the portal is done apart.
pub struct Shared(pub Arc<Host>);

impl meshctld::Host for Shared {
    fn keep_running(&self, on: bool) {
        self.0.keep.store(on, Ordering::Relaxed);
    }

    fn notify(&self, notification: &Notification) {
        let host = Arc::clone(&self.0);
        let notification = notification.clone();
        tauri::async_runtime::spawn(async move {
            let Some(portal) = host.portal().await else {
                return;
            };
            let id = id(notification.radio, notification.peer);
            let chat = serde_json::json!({
                "radio": notification.radio,
                "peer": notification.peer,
            })
            .to_string();
            let mut title = notification.sender.clone();
            if let Some(channel) = &notification.channel {
                title.push_str(&format!(" · #{channel}"));
            }
            title.push_str(&format!(" · {}", notification.radio_name));
            let body = notification.text.as_deref().unwrap_or(host.new_message);
            let shown = Shown::new(&title)
                .body(body)
                .category(Category::ImMessage)
                .priority(Priority::High)
                .default_action(OPEN);
            if let Ok(mut kept) = host.shown.lock() {
                kept.insert(id.clone(), chat);
            }
            if let Err(error) = portal.add_notification(&id, shown).await {
                tracing::warn!("cannot notify: {error}");
            }
        });
    }

    fn read(&self, radio: RadioId, peer: Peer) {
        let host = Arc::clone(&self.0);
        let id = id(radio, peer);
        let was_shown = host
            .shown
            .lock()
            .is_ok_and(|mut shown| shown.remove(&id).is_some());
        if !was_shown {
            return;
        }
        tauri::async_runtime::spawn(async move {
            if let Some(portal) = host.portal().await {
                let _ = portal.remove_notification(&id).await;
            }
        });
    }
}
