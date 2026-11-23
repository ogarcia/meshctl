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
mod links;
mod map;
mod mesh;
mod meshcore_channels;
mod meshcore_settings;
mod names;
mod node;
mod offline;
mod packets;
mod phone;
mod preferences;
mod radios;
mod sections;
mod settings;
mod storage;
mod traces;
mod welcome;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
