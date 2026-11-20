// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Connects to a MeshCore companion radio, says it is meshctl, and prints who
//! the radio is, its contacts and its channels. Read only: it sends nothing
//! over the air, and leaves the radio's queue of messages as it is.
//!
//!     cargo run -p meshctl-core --example meshcore_probe -- 192.168.1.10 5050

use meshcore_rs::{EventPayload, EventType, MeshCore};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: meshcore_probe <host> [port]")?;
    let port: u16 = args.next().map_or(Ok(5000), |p| p.parse())?;
    let client = MeshCore::tcp(&host, port).await?;

    // APP_START, named: the command byte, seven reserved bytes, the name.
    let mut frame = vec![0u8; 8];
    frame[0] = 0x01;
    frame.extend_from_slice(b"meshctl");
    let commands = client.commands();
    let answer = commands
        .lock()
        .await
        .send(&frame, Some(EventType::SelfInfo))
        .await?;
    let EventPayload::SelfInfo(me) = answer.payload else {
        return Err(format!("APP_START answered with {:?}", answer.payload).into());
    };
    println!(
        "self: {:?} type {} key {} freq {} bw {} sf {} cr {} tx {}/{} lat {} lon {}",
        me.name,
        me.adv_type,
        hex(&me.public_key[..8]),
        me.radio_freq,
        me.radio_bw,
        me.sf,
        me.cr,
        me.tx_power,
        me.max_tx_power,
        me.adv_lat,
        me.adv_lon,
    );

    match commands.lock().await.send_device_query().await {
        Ok(device) => println!(
            "device: model {:?} version {:?} build {:?} max contacts {:?} channels {:?}",
            device.model, device.version, device.fw_build, device.max_contacts, device.max_channels
        ),
        Err(error) => println!("device query failed: {error}"),
    }

    let contacts = commands.lock().await.get_contacts(0).await?;
    println!("{} contacts", contacts.len());
    let mut by_type = std::collections::BTreeMap::<u8, usize>::new();
    for contact in &contacts {
        *by_type.entry(contact.contact_type).or_default() += 1;
    }
    println!("by type: {by_type:?}");
    for contact in contacts.iter().take(6) {
        println!(
            "  {} {:?} type {} path {} last advert {} lat {} lon {}",
            hex(&contact.public_key[..8]),
            contact.adv_name,
            contact.contact_type,
            contact.path_len,
            contact.last_advert,
            contact.adv_lat,
            contact.adv_lon,
        );
    }

    for index in 0..16u8 {
        match commands.lock().await.get_channel(index).await {
            Ok(channel) if !channel.name.is_empty() => {
                println!("channel {index}: {:?}", channel.name);
            }
            Ok(_) => {}
            Err(error) => {
                println!("channel {index}: {error}");
                break;
            }
        }
    }

    client.disconnect().await?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
