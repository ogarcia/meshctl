// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Connects to a MeshCore companion radio, says it is meshctl, and prints who
//! the radio is, its contacts and its channels. Read only: it sends nothing
//! over the air, and leaves the radio's queue of messages as it is.
//!
//!     cargo run -p meshctl-core --example meshcore_probe -- 192.168.1.10 5050

use meshctl_core::link::Transport;
use meshctl_core::meshcore::{Client, DEFAULT_TCP_PORT};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: meshcore_probe <host> [port]")?;
    let port: u16 = args.next().map_or(Ok(DEFAULT_TCP_PORT), |p| p.parse())?;
    let (client, _pushes) = Client::connect(&Transport::Tcp { host, port }).await?;

    let me = client.start().await?;
    println!(
        "self: {:?} type {} key {} freq {} kHz bw {} Hz sf {} cr {} tx {}/{}",
        me.name,
        me.kind,
        hex(&me.public_key[..8]),
        me.frequency,
        me.bandwidth,
        me.spreading_factor,
        me.coding_rate,
        me.tx_power,
        me.max_tx_power,
    );

    let device = client.device().await;
    println!("device: {device:?}");
    println!("extras: {:?}", client.extras().await);

    let (contacts, latest) = client.contacts(0).await?;
    println!("{} contacts, latest change {latest}", contacts.len());
    let mut by_kind = std::collections::BTreeMap::<u8, usize>::new();
    for contact in &contacts {
        *by_kind.entry(contact.kind).or_default() += 1;
    }
    println!("by kind: {by_kind:?}");
    for contact in contacts.iter().take(4) {
        println!(
            "  {} {:?} kind {} path {} modified {}",
            hex(&contact.public_key[..8]),
            contact.name,
            contact.kind,
            contact.path_len,
            contact.last_modified,
        );
    }

    let slots = device.and_then(|d| d.max_channels).unwrap_or(8);
    for (index, name, _) in client.channels(slots).await {
        println!("channel {index}: {name:?}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
