// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl as a desktop app: the app itself is the library's, which a
//! phone's app runs too.

// No console window on Windows, but while developing.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() {
    meshctl_app::run();
}
