// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a desktop's notifications of messages have alike, however each
//! system shows them: one for each conversation, by an id of its own, with
//! who wrote and where, and what it says; picked, the window shows on its
//! conversation.

use meshctl_api::{Notification, Peer, RadioId};
use tauri::{AppHandle, Manager};

/// A conversation's notification's id: its radio, and its channel or node.
pub fn id(radio: RadioId, peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => format!("meshctl-{radio}-c{channel}"),
        Peer::Node { node } => format!("meshctl-{radio}-n{node}"),
    }
}

/// A notification's conversation, as the panel takes it to open it.
pub fn chat(notification: &Notification) -> String {
    serde_json::json!({ "radio": notification.radio, "peer": notification.peer }).to_string()
}

/// Who wrote, where, and through which radio.
pub fn title(notification: &Notification) -> String {
    let mut title = notification.sender.clone();
    if let Some(channel) = &notification.channel {
        title.push_str(&format!(" · #{channel}"));
    }
    title.push_str(&format!(" · {}", notification.radio_name));
    title
}

/// What it says, or, not to be shown, that there is a message: in the
/// system's language.
pub fn body(notification: &Notification) -> &str {
    notification.text.as_deref().unwrap_or_else(|| {
        if sys_locale::get_locale().is_some_and(|locale| locale.starts_with("es")) {
            "Mensaje nuevo"
        } else {
            "New message"
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
