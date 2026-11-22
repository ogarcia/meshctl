// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Copying a text to the clipboard.

use wasm_bindgen::JsCast;

/// Puts a text on the clipboard: by the clipboard's own API where the page
/// may use it, which is only over HTTPS or on this machine; otherwise by
/// copying it from a text area put on the page for a moment.
pub fn copy(text: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    if window.is_secure_context() {
        let _ = window.navigator().clipboard().write_text(text);
        return;
    }
    let Some(document) = window.document() else {
        return;
    };
    let Some(area) = document
        .create_element("textarea")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
    else {
        return;
    };
    area.set_value(text);
    let _ = area.set_attribute("readonly", "");
    let _ = area.set_attribute("style", "position: fixed; top: -1000px; opacity: 0");
    let Some(body) = document.body() else {
        return;
    };
    let _ = body.append_child(&area);
    area.select();
    if let Ok(document) = document.dyn_into::<web_sys::HtmlDocument>() {
        let _ = document.exec_command("copy");
    }
    area.remove();
}
