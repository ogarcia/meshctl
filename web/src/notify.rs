// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Notifications, in the browser: the daemon says what to notify of, on the
//! event stream, and the page shows it where the browser lets it. The page
//! tells the daemon, in turn, the conversation it shows while it is seen,
//! for nothing in it to be notified of.
//!
//! Within an app, the app shows them on its own, with its window closed
//! too: the page shows none, and opens the conversation of one picked
//! there, as the app tells it.

use crate::app::{Live, Page};
use crate::daemon::{self, Teller};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::{ClientMessage, Viewing};
use meshctl_api::{Notification, Peer, RadioId};
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
    // A notification picked in the app while the page shows.
    std::mem::forget(window_event_listener_untyped(
        "meshctl-chat",
        move |event| {
            if let Some(chat) = event
                .dyn_ref::<web_sys::CustomEvent>()
                .and_then(|e| e.detail().as_string())
            {
                open_chat(live, &chat);
            }
        },
    ));
}

/// The daemon reached: a notification picked in the app as it started, its
/// conversation opened.
pub fn ready(live: Live) {
    let window = window();
    let Some(app) = js_sys::Reflect::get(&window, &JsValue::from_str("meshctlApp"))
        .ok()
        .filter(|app| !app.is_undefined() && !app.is_null())
    else {
        return;
    };
    let take = js_sys::Reflect::get(&app, &JsValue::from_str("takeChat"))
        .ok()
        .and_then(|take| take.dyn_into::<js_sys::Function>().ok());
    if let Some(chat) = take
        .and_then(|take| take.call0(&app).ok())
        .and_then(|chat| chat.as_string())
    {
        open_chat(live, &chat);
    }
}

/// A conversation to open, as the app gives it, in JSON; or a waypoint, to
/// show on the map.
#[derive(serde::Deserialize)]
struct Chat {
    radio: RadioId,
    peer: Peer,
    #[serde(default)]
    waypoint: Option<u32>,
}

fn open_chat(live: Live, chat: &str) {
    let Ok(chat) = serde_json::from_str::<Chat>(chat) else {
        return;
    };
    if live.selected.get_untracked() != Some(chat.radio) {
        live.select(chat.radio);
    }
    match chat.waypoint {
        Some(id) => live.show_waypoint(id),
        None => live.open_chat(chat.peer),
    }
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
    let body = notification.waypoint.as_ref().map_or_else(
        || notification.text.clone().unwrap_or_default(),
        |waypoint| waypoint_body(live, waypoint.event, notification.text.as_deref()),
    );
    options.set_body(&body);
    // One for each conversation, the newest in place of the one before; a
    // waypoint's, one of its own.
    options.set_tag(&notification.waypoint.as_ref().map_or_else(
        || {
            format!(
                "meshctl-{}-{}",
                notification.radio,
                peer_key(notification.peer)
            )
        },
        |waypoint| format!("meshctl-{}-w{}", notification.radio, waypoint.id),
    ));
    options.set_icon("/favicon.svg");
    let Ok(shown) = web_sys::Notification::new_with_options(&title, &options) else {
        return;
    };
    let chat = serde_json::json!({
        "radio": notification.radio,
        "peer": notification.peer,
        "waypoint": notification.waypoint.as_ref().map(|w| w.id),
    })
    .to_string();
    let closing = shown.clone();
    let open = Closure::once_into_js(move || {
        let _ = window().focus();
        closing.close();
        open_chat(live, &chat);
    });
    shown.set_onclick(Some(open.unchecked_ref()));
}

/// What happened to a waypoint, in the interface's language, and to which.
fn waypoint_body(live: Live, event: meshctl_api::WaypointEvent, text: Option<&str>) -> String {
    use crate::i18n::{Locale, td_string};
    use meshctl_api::WaypointEvent;
    let locale: Locale = untrack(|| live.locale());
    let said = match event {
        WaypointEvent::Shared => td_string!(locale, notify_waypoint_shared),
        WaypointEvent::Entered => td_string!(locale, notify_waypoint_entered),
        WaypointEvent::Left => td_string!(locale, notify_waypoint_left),
    };
    match text {
        Some(text) if event == WaypointEvent::Shared => format!("{said}: {text}"),
        Some(text) => format!("{said} {text}"),
        None => said.to_owned(),
    }
}

fn peer_key(peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => format!("c{channel}"),
        Peer::Node { node } => format!("n{node}"),
    }
}
