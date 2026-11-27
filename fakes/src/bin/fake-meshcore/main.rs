// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend MeshCore companion radio on TCP, for working on meshctl without
//! one, or without risking one: it speaks the companion protocol as the
//! firmware does, keeps its contacts and channels, queues the messages it
//! hears to be pulled, and acknowledges direct messages sent to it. It
//! transmits nothing, anywhere.
//!
//!     cargo run -p meshctl-fakes --bin fake-meshcore -- [port] [control port]
//!
//! What it does, its contacts, and the lines its control port takes, are in
//! the crate's README.

mod commands;
mod control;
mod crowd;
mod frames;
mod link;
mod nodes;
mod radio;

use crate::radio::Radio;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (port, control) = meshctl_fakes::ports(5000)?;
    let radio = Arc::new(Mutex::new(Radio::new()));
    let (events, _) = broadcast::channel(64);
    let serve = {
        let (radio, events) = (Arc::clone(&radio), events.clone());
        move |stream| link::serve(stream, Arc::clone(&radio), events.clone())
    };
    let answer = move |line: &str| control::control(line, &radio, &events);
    meshctl_fakes::run(
        port,
        control,
        &format!("fake MeshCore radio on port {port}"),
        serve,
        answer,
    )
    .await
}
