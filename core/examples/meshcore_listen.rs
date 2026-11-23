// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Listens to what a MeshCore radio hears, raw, for a while, and prints each
//! packet: how it came, its region code, its path, and a channel message's
//! text when the radio has its channel. Read only: it sends nothing over
//! the air.
//!
//!     cargo run -p meshctl-core --example meshcore_listen -- 192.168.1.10 5050 120

use std::time::Duration;

use meshctl_core::link::Transport;
use meshctl_core::meshcore::protocol::Response;
use meshctl_core::meshcore::raw::{self, RawPacket};
use meshctl_core::meshcore::{Client, DEFAULT_TCP_PORT};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: meshcore_listen <host> [port] [seconds]";
    let host = args.next().ok_or(usage)?;
    let port: u16 = args.next().map_or(Ok(DEFAULT_TCP_PORT), |p| p.parse())?;
    let seconds: u64 = args.next().map_or(Ok(120), |s| s.parse())?;
    let (client, mut pushes) = Client::connect(&Transport::Tcp { host, port }).await?;
    client.start().await?;
    let slots = client
        .device()
        .await
        .and_then(|d| d.max_channels)
        .unwrap_or(8);
    let channels = client.channels(slots).await;
    println!("listening {seconds} s, {} channels", channels.len());
    let listen = async {
        while let Some(push) = pushes.recv().await {
            let Response::RxLog { snr, rssi, raw } = push else {
                continue;
            };
            let Some(packet) = RawPacket::parse(&raw) else {
                println!("unreadable: {}", hex(&raw));
                continue;
            };
            let hops: Vec<String> = packet.hops().iter().map(|h| hex(h)).collect();
            let mut line = format!(
                "{snr:5.1} dB {rssi:4} dBm route {} type {:#04x} key {:016x} path [{}]",
                packet.route,
                packet.payload_type,
                packet.key(),
                hops.join(" ")
            );
            if let Some(codes) = packet.transport {
                line.push_str(&format!(" region {:04x}", codes[0]));
            }
            if packet.payload_type == raw::PAYLOAD_GRP_TXT {
                let opened = channels.iter().find_map(|(index, name, secret)| {
                    raw::open_channel_text(secret, &packet.payload)
                        .map(|(at, text)| format!(" #{index} {name:?} at {at}: {text:?}"))
                });
                line.push_str(&opened.unwrap_or_else(|| " (a channel not ours)".to_owned()));
            }
            if packet.payload_type == raw::PAYLOAD_TXT_MSG {
                line.push_str(&format!(
                    " to {:02x} from {:02x}",
                    packet.payload.first().copied().unwrap_or(0),
                    packet.payload.get(1).copied().unwrap_or(0)
                ));
            }
            println!("{line}");
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(seconds), listen).await;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
