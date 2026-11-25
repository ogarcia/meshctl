// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Notifications, in the browser: the daemon says what to notify of, on the
//! event stream, and the page shows it where the browser lets it. The page
//! tells the daemon, in turn, the conversation it shows while it is seen,
//! for nothing in it to be notified of.
//!
//! Within an app, the app shows them on its own, with its window closed
//! too: the page shows none.

use crate::app::{Live, Page};
use crate::daemon::{self, Teller};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::{ClientMessage, Viewing};
use meshctl_api::{Notification, Peer};
use std::cell::{Cell, RefCell};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

thread_local! {
    /// How to tell the daemon, while the event stream is open.
    static TELLER: RefCell<Option<Teller>> = const { RefCell::new(None) };
    /// Whether the page is within an app, which shows notifications itself.
    static IN_APP: Cell<bool> = const { Cell::new(false) };
    /// Whether the page is seen: shown, and the window in front.
    static SEEN: Cell<Option<RwSignal<bool>>> = const { Cell::new(None) };
}

/// What the browser lets of notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    /// Not asked yet.
    Ask,
    /// A browser without them, or a page not served over HTTPS.
    Unsupported,
}

/// Follows whether the page is seen, and tells the daemon what it shows
/// whenever it changes: once, as the page starts.
pub fn watch(live: Live) {
    let seen = RwSignal::new(is_seen());
    SEEN.set(Some(seen));
    for event in ["visibilitychange", "focus", "blur"] {
        // For as long as the page lives.
        std::mem::forget(window_event_listener_untyped(event, move |_| {
            seen.set(is_seen());
        }));
    }
    Effect::new(move |_| tell(viewing(live, seen)));
}

/// A new event stream: the daemon told what the page shows, as it is now,
/// and whether the page is within an app read again.
pub fn connected(live: Live, teller: Teller) {
    TELLER.with(|t| *t.borrow_mut() = Some(teller));
    if let Some(seen) = SEEN.get() {
        tell(untrack(|| viewing(live, seen)));
    }
    spawn_local(async {
        if let Ok(settings) = daemon::app_settings().await {
            IN_APP.set(settings.background.is_some());
        }
    });
}

/// Whether the page is within an app, as far as is known.
pub fn in_app() -> bool {
    IN_APP.get()
}

fn is_seen() -> bool {
    let document = document();
    !document.hidden() && document.has_focus().unwrap_or(true)
}

/// The conversation shown, while the page is seen.
fn viewing(live: Live, seen: RwSignal<bool>) -> Option<Viewing> {
    if live.page.get() != Page::Chat || !seen.get() {
        return None;
    }
    Some(Viewing {
        radio: live.selected.get()?,
        peer: live.open.get()?,
    })
}

fn tell(viewing: Option<Viewing>) {
    let Some(teller) = TELLER.with(|t| t.borrow().clone()) else {
        return;
    };
    spawn_local(async move {
        teller.tell(&ClientMessage::Viewing { viewing }).await;
    });
}

/// What the browser lets now.
pub fn permission() -> Permission {
    let window = window();
    if !js_sys::Reflect::has(&window, &JsValue::from_str("Notification")).unwrap_or(false) {
        return Permission::Unsupported;
    }
    match web_sys::Notification::permission() {
        web_sys::NotificationPermission::Granted => Permission::Granted,
        web_sys::NotificationPermission::Denied => Permission::Denied,
        _ => Permission::Ask,
    }
}

/// Asks the browser to let the page notify, as something the user did
/// asks it; what it lets, after.
pub async fn ask() -> Permission {
    if permission() != Permission::Ask {
        return permission();
    }
    if let Ok(promise) = web_sys::Notification::request_permission() {
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }
    permission()
}

/// Shows a notification the daemon sent, where the browser lets it and the
/// page is not within an app; on it, the page comes to the front, in its
/// conversation.
pub fn show(live: Live, notification: Notification) {
    if in_app() || permission() != Permission::Granted {
        return;
    }
    let many_radios = live
        .radios
        .with_untracked(|r| r.as_ref().is_some_and(|r| r.len() > 1));
    let mut title = notification.sender.clone();
    if let Some(channel) = &notification.channel {
        title.push_str(&format!(" · #{channel}"));
    }
    if many_radios {
        title.push_str(&format!(" · {}", notification.radio_name));
    }
    let options = web_sys::NotificationOptions::new();
    options.set_body(notification.text.as_deref().unwrap_or_default());
    // One for each conversation, the newest in place of the one before.
    options.set_tag(&format!(
        "meshctl-{}-{}",
        notification.radio,
        peer_key(notification.peer)
    ));
    options.set_icon("/favicon.svg");
    let Ok(shown) = web_sys::Notification::new_with_options(&title, &options) else {
        return;
    };
    let (radio, peer) = (notification.radio, notification.peer);
    let closing = shown.clone();
    let open = Closure::once_into_js(move || {
        let _ = window().focus();
        closing.close();
        if live.selected.get_untracked() != Some(radio) {
            live.select(radio);
        }
        live.open_chat(peer);
    });
    shown.set_onclick(Some(open.unchecked_ref()));
}

fn peer_key(peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => format!("c{channel}"),
        Peer::Node { node } => format!("n{node}"),
    }
}
