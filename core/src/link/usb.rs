// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio plugged in by USB where the system gives no serial device for
//! it, as on Android: the app meshctl runs within opens it, through the
//! system's own USB, and hands over a stream of its bytes, which goes as a
//! serial port's would.

use super::{Framing, Link};
use meshctl_proto::frame;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// A stream of a radio's bytes, both ways.
pub trait UsbStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> UsbStream for T {}

/// What opening a radio by USB comes to: its stream, or why not.
pub type Opening = Pin<Box<dyn Future<Output = io::Result<Box<dyn UsbStream>>> + Send>>;

/// A radio by USB, as the app tells it: its vendor's and product's ids,
/// its maker and product if it says, and whether meshctl may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub vendor: u16,
    pub product_id: u16,
    pub maker: Option<String>,
    pub product: Option<String>,
    pub allowed: bool,
}

/// The app's way to open radios by USB, and to tell them.
pub trait UsbOpener: Send + Sync {
    /// Opens the one it is given, as its vendor's and product's ids,
    /// `vvvv:pppp` in hexadecimal, or the first there is; at a speed.
    fn open(&self, device: Option<String>, baud: u32) -> Opening;

    /// Those plugged in now.
    fn list(&self) -> Pin<Box<dyn Future<Output = Vec<UsbDevice>> + Send>>;

    /// Asks the user for leave to use one, by its ids; whether it has it.
    fn ask(&self, device: String) -> Pin<Box<dyn Future<Output = bool> + Send>>;
}

static OPENER: OnceLock<Arc<dyn UsbOpener>> = OnceLock::new();

/// Takes the app's way to open radios by USB: once, as it starts.
pub fn set_opener(opener: Arc<dyn UsbOpener>) {
    let _ = OPENER.set(opener);
}

/// Whether there is a way to open radios by USB here.
pub fn available() -> bool {
    OPENER.get().is_some()
}

/// The radios plugged in by USB, as the app tells them.
pub async fn list() -> Vec<super::plugged::Plugged> {
    let Some(opener) = OPENER.get() else {
        return Vec::new();
    };
    opener
        .list()
        .await
        .into_iter()
        .map(|device| {
            let ids = format!("{:04x}:{:04x}", device.vendor, device.product_id);
            super::plugged::Plugged {
                address: format!("usb://{ids}"),
                ids: Some(ids),
                maker: device.maker,
                product: device.product,
                chip: super::plugged::chip(device.vendor),
                ready: device.allowed,
            }
        })
        .collect()
}

/// Asks the user for leave to use a radio by USB, by its address as
/// `usb://vvvv:pppp`; whether meshctl has it. None where there is no app
/// to ask.
pub async fn ask(address: &str) -> Option<bool> {
    let opener = OPENER.get()?;
    let Ok(super::Transport::Usb {
        device: Some(device),
        ..
    }) = address.parse::<super::Transport>()
    else {
        return Some(false);
    };
    Some(opener.ask(device).await)
}

pub async fn connect(device: Option<&str>, baud: u32, framing: Framing) -> io::Result<Link> {
    let Some(opener) = OPENER.get() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "radios by USB are opened by an app, as Android's: here, give the serial device's path",
        ));
    };
    let mut stream = opener.open(device.map(str::to_owned), baud).await?;
    if framing == Framing::Meshtastic {
        // As on a serial port: a run of START2 wakes a sleeping device and
        // gets the firmware out of the middle of a half-read frame.
        stream.write_all(&[frame::START2; 32]).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(super::spawn(stream, "usb", framing))
}
