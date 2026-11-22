// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"));

mod app;
mod badge;
mod channel_settings;
mod charts;
mod chat;
mod clipboard;
mod daemon;
mod emoji;
mod empty;
mod fields;
mod map;
mod mesh;
mod names;
mod node;
mod packets;
mod phone;
mod preferences;
mod radios;
mod sections;
mod settings;
mod traces;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
