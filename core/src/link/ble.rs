// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio by Bluetooth, as the app meshctl runs within reaches it, as on
//! Android: not a stream of bytes but its payloads, one by one, as each
//! firmware's service carries them (Meshtastic's to and from the radio,
//! MeshCore's over Nordic's UART), so that they go as a link's do as they
//! are.

use super::{Framing, Link};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

/// A radio by Bluetooth's payloads: those to send, and those it sends,
/// which end as it goes.
pub type Pipe = (
    mpsc::UnboundedSender<Vec<u8>>,
    mpsc::UnboundedReceiver<Vec<u8>>,
);

/// A radio heard nearby, by Bluetooth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nearby {
    /// Its Bluetooth address, `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    pub name: Option<String>,
    /// How well it is heard, in dBm.
    pub rssi: Option<i16>,
    /// Its firmware, by the service it says it has.
    pub framing: Option<Framing>,
}

/// What looking for radios nearby comes to: those heard, or that the
/// user is yet to let meshctl use Bluetooth, which the app is asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Looked {
    Heard(Vec<Nearby>),
    Asked,
}

/// The app's way to reach radios by Bluetooth.
pub trait BleOpener: Send + Sync {
    /// Connects to one, by its address, for the firmware it has.
    fn open(
        &self,
        address: String,
        framing: Framing,
    ) -> Pin<Box<dyn Future<Output = io::Result<Pipe>> + Send>>;

    /// Looks for radios nearby, a few seconds.
    fn look(&self) -> Pin<Box<dyn Future<Output = Looked> + Send>>;
}

static OPENER: OnceLock<Arc<dyn BleOpener>> = OnceLock::new();

/// Takes the app's way to reach radios by Bluetooth: once, as it starts.
pub fn set_opener(opener: Arc<dyn BleOpener>) {
    let _ = OPENER.set(opener);
}

/// Whether there is a way to reach radios by Bluetooth here.
pub fn available() -> bool {
    OPENER.get().is_some()
}

/// Looks for radios nearby; none where there is no way to.
pub async fn look() -> Option<Looked> {
    Some(OPENER.get()?.look().await)
}

/// Whether an address is a Bluetooth one, `AA:BB:CC:DD:EE:FF`.
pub fn is_address(address: &str) -> bool {
    let parts: Vec<&str> = address.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

pub async fn connect(address: &str, framing: Framing) -> io::Result<Link> {
    let Some(opener) = OPENER.get() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "radios by Bluetooth are reached by an app, as Android's",
        ));
    };
    let (tx, rx) = opener.open(address.to_owned(), framing).await?;
    Ok(Link::from_pipe(tx, rx))
}
