// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios plugged in where meshctl runs, to pick one from rather than
//! know its address: by USB through the app it runs within, where there is
//! one, as on Android; or its serial ports that are USB ones.

use super::usb;

/// A radio plugged in, as far as it tells: what to give as its address,
/// and what to know it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugged {
    /// Its address as a radio's: a serial device's path, or `usb://…`.
    pub address: String,
    /// Its vendor's and product's ids, `vvvv:pppp`, if it says.
    pub ids: Option<String>,
    pub maker: Option<String>,
    pub product: Option<String>,
    /// Its USB chip, by its vendor, if known.
    pub chip: Option<&'static str>,
    /// Whether it can be opened now: not, by USB, until the user lets
    /// meshctl use it.
    pub ready: bool,
}

/// What a USB serial chip is, by its vendor's id, of those radios come
/// with: the chip's own, or the board maker's on its own USB.
pub fn chip(vendor: u16) -> Option<&'static str> {
    Some(match vendor {
        0x10c4 => "CP210x",
        0x1a86 => "CH34x",
        0x0403 => "FTDI",
        0x303a => "ESP32",
        0x239a => "nRF52 (Adafruit)",
        0x2886 => "nRF52 (Seeed)",
        0x1915 => "nRF52",
        0x2e8a => "RP2040",
        _ => return None,
    })
}

/// The radios plugged in now: those the app opens by USB, where it does;
/// or else the serial ports that are USB ones, a desktop's.
pub async fn plugged() -> Vec<Plugged> {
    if usb::available() {
        return usb::list().await;
    }
    tokio::task::spawn_blocking(serial_ports)
        .await
        .unwrap_or_default()
}

fn serial_ports() -> Vec<Plugged> {
    let Ok(ports) = tokio_serial::available_ports() else {
        return Vec::new();
    };
    ports
        .into_iter()
        // On macOS each comes twice: the call-out device is the one to open.
        .filter(|port| !port.port_name.starts_with("/dev/tty.") || !cfg!(target_os = "macos"))
        .filter_map(|port| match port.port_type {
            tokio_serial::SerialPortType::UsbPort(info) => Some(Plugged {
                address: port.port_name,
                ids: Some(format!("{:04x}:{:04x}", info.vid, info.pid)),
                maker: info.manufacturer,
                product: info.product,
                chip: chip(info.vid),
                ready: true,
            }),
            _ => None,
        })
        .collect()
}
