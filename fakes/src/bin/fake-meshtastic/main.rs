// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend Meshtastic radio on TCP, for working on meshctl without one, or
//! without risking one: it sends every part of the configuration and every
//! module these protobufs know, answers admin requests with a passkey, and
//! takes in what it is told to change, so that reading it back shows it.
//! Its one neighbour answers what it is asked for. It transmits nothing,
//! anywhere.
//!
//!     cargo run -p meshctl-fakes --bin fake-meshtastic -- [port] [control port]
//!
//! What it does, and the lines its control port takes, are in the crate's
//! README.

mod control;
mod crowd;
mod link;
mod neighbour;
mod radio;

use crate::radio::{ME, Radio};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (port, control) = meshctl_fakes::ports(4403)?;
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
        &format!("fake radio !{ME:08x} on port {port}"),
        serve,
        answer,
    )
    .await
}
