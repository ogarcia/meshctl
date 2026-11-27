// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a desktop's notifications of messages have alike, however each
//! system shows them: one for each conversation, by an id of its own, with
//! who wrote and where, and what it says; picked, the window shows on its
//! conversation.

use meshctl_api::{Notification, Peer, RadioId, WaypointEvent};
use tauri::{AppHandle, Manager};

/// A conversation's notification's id: its radio, and its channel or node.
pub fn id(radio: RadioId, peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => format!("meshctl-{radio}-c{channel}"),
        Peer::Node { node } => format!("meshctl-{radio}-n{node}"),
    }
}

/// A notification's own id: its conversation's, but a waypoint's own, and
/// a full radio's, so that they do not take the place of a message's.
pub fn notice_id(notification: &Notification) -> String {
    if notification.contacts_full {
        return format!("meshctl-{}-full", notification.radio);
    }
    match &notification.waypoint {
        Some(waypoint) => format!("meshctl-{}-w{}", notification.radio, waypoint.id),
        None => id(notification.radio, notification.peer),
    }
}

/// A notification's conversation, as the panel takes it to open it: for a
/// waypoint, the waypoint, which opens on the map; for a full radio, its
/// settings.
pub fn chat(notification: &Notification) -> String {
    serde_json::json!({
        "radio": notification.radio,
        "peer": notification.peer,
        "waypoint": notification.waypoint.as_ref().map(|w| w.id),
        "full": notification.contacts_full,
    })
    .to_string()
}

/// Who wrote, where, and through which radio; a full radio, its name.
pub fn title(notification: &Notification) -> String {
    if notification.contacts_full {
        return notification.radio_name.clone();
    }
    let mut title = notification.sender.clone();
    if let Some(channel) = &notification.channel {
        title.push_str(&format!(" · #{channel}"));
    }
    title.push_str(&format!(" · {}", notification.radio_name));
    title
}

/// What it says, or, not to be shown, that there is a message; of a
/// waypoint, what happened to it: in the system's language.
pub fn body(notification: &Notification) -> String {
    let spanish = sys_locale::get_locale().is_some_and(|locale| locale.starts_with("es"));
    if notification.contacts_full {
        return if spanish {
            "Su lista de contactos está llena: los nodos nuevos que oye no se guardan."
        } else {
            "Its list of contacts is full: the new nodes it hears are not kept."
        }
        .to_owned();
    }
    if let Some(waypoint) = &notification.waypoint {
        let said = match (waypoint.event, spanish) {
            (WaypointEvent::Shared, false) => "Shared a waypoint",
            (WaypointEvent::Shared, true) => "Compartió un waypoint",
            (WaypointEvent::Entered, false) => "Went into",
            (WaypointEvent::Entered, true) => "Entró en",
            (WaypointEvent::Left, false) => "Left",
            (WaypointEvent::Left, true) => "Salió de",
        };
        return match &notification.text {
            Some(text) if waypoint.event == WaypointEvent::Shared => format!("{said}: {text}"),
            Some(text) => format!("{said} {text}"),
            None => said.to_owned(),
        };
    }
    notification.text.clone().unwrap_or_else(|| {
        if spanish {
            "Mensaje nuevo".to_owned()
        } else {
            "New message".to_owned()
        }
    })
}

/// Shows the window, and tells its panel to open a conversation.
pub fn open(app: &AppHandle, chat: &str) {
    crate::show(app);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let detail = serde_json::to_string(chat).unwrap_or_default();
    let _ = window.eval(format!(
        "window.dispatchEvent(new CustomEvent('meshctl-chat', {{ detail: {detail} }}))"
    ));
}
