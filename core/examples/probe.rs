// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Connects to a radio, asks for its configuration, and prints a line for
//! everything it sends back, then keeps listening for a while. Read only: it
//! never transmits over the air.
//!
//!     cargo run -p meshctl-core --example probe -- 192.168.1.10 4403 [seconds]

use meshctl_core::link::{self, Transport};
use meshctl_proto::meshtastic::{FromRadio, ToRadio, from_radio, mesh_packet, to_radio};
use prost::Message;
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: probe <host> <port> [seconds]")?;
    let port = args
        .next()
        .map_or(Ok(link::DEFAULT_TCP_PORT), |p| p.parse())?;
    let listen = args.next().map_or(Ok(30), |s| s.parse())?;

    let mut link = link::connect(&Transport::Tcp { host, port }).await?;
    let nonce = 0x6d63_746c; // "mctl"
    link.tx.send(
        ToRadio {
            payload_variant: Some(to_radio::PayloadVariant::WantConfigId(nonce)),
        }
        .encode_to_vec(),
    )?;

    let deadline = tokio::time::sleep(Duration::from_secs(listen));
    tokio::pin!(deadline);
    loop {
        let payload = tokio::select! {
            payload = link.rx.recv() => match payload {
                Some(payload) => payload,
                None => { println!("link closed"); break; }
            },
            () = &mut deadline => break,
        };
        let message = FromRadio::decode(payload.as_slice())?;
        match message.payload_variant {
            Some(from_radio::PayloadVariant::Packet(packet)) => {
                let port = match &packet.payload_variant {
                    Some(mesh_packet::PayloadVariant::Decoded(data)) => {
                        format!("port {}", data.portnum)
                    }
                    Some(mesh_packet::PayloadVariant::Encrypted(_)) => "encrypted".into(),
                    None => "empty".into(),
                };
                println!(
                    "packet !{:08x} -> !{:08x} ch {} {port} snr {} rssi {:?} hops {}/{}",
                    packet.from,
                    packet.to,
                    packet.channel,
                    packet.rx_snr,
                    packet.rx_rssi,
                    packet.hop_start.saturating_sub(packet.hop_limit),
                    packet.hop_start,
                );
            }
            Some(from_radio::PayloadVariant::NodeInfo(node)) => {
                let user = node.user.unwrap_or_default();
                println!(
                    "node !{:08x} {:?}/{:?} hw {} role {} last_heard {} snr {} hops {:?} pos {}",
                    node.num,
                    user.long_name,
                    user.short_name,
                    user.hw_model,
                    user.role,
                    node.last_heard,
                    node.snr,
                    node.hops_away,
                    node.position.is_some(),
                );
            }
            Some(from_radio::PayloadVariant::ConfigCompleteId(id)) => {
                println!(
                    "config complete ({})",
                    if id == nonce { "ours" } else { "not ours" }
                );
            }
            Some(other) => {
                let text = format!("{other:?}");
                println!("{}", &text[..text.len().min(220)]);
            }
            None => println!("empty FromRadio"),
        }
    }
    Ok(())
}
