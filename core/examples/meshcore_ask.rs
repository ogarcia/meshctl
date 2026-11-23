// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Asks a repeater a MeshCore radio knows, by part of its name, which regions it
//! repeats, and prints what it answers. This goes on the air: one direct
//! request to that repeater, and nothing else.
//!
//!     cargo run -p meshctl-core --example meshcore_ask -- 192.168.1.10 5050 StaLocaia

use std::time::Duration;

use meshctl_core::link::Transport;
use meshctl_core::meshcore::Client;
use meshctl_core::meshcore::protocol::{Response, command, path_bytes};

/// Asks for the regions it repeats on.
const REGIONS: u8 = 0x01;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: meshcore_ask <host> <port> <name>";
    let host = args.next().ok_or(usage)?;
    let port: u16 = args.next().ok_or(usage)?.parse()?;
    let name = args.next().ok_or(usage)?;
    let (client, mut pushes) = Client::connect(&Transport::Tcp { host, port }).await?;
    client.start().await?;
    let (contacts, _) = client.contacts(0).await?;
    let contact = contacts
        .iter()
        .find(|c| c.name.to_lowercase().contains(&name.to_lowercase()))
        .ok_or("no contact by that name")?;
    let path_len = contact.path_len.cast_unsigned();
    println!(
        "{:?} kind {} path_len {:#04x} path {}",
        contact.name,
        contact.kind,
        path_len,
        hex(&contact.path)
    );
    if path_len == 0xff {
        println!("no path known: the request would flood, and a repeater answers only direct ones");
        return Ok(());
    }
    // The way back: the same hashes, the other way round.
    let size = usize::from(path_len >> 6) + 1;
    let mut back: Vec<u8> = Vec::with_capacity(path_bytes(path_len));
    for hash in contact.path.chunks(size).rev() {
        back.extend_from_slice(hash);
    }
    let mut request = vec![REGIONS, path_len];
    request.extend_from_slice(&back);
    let sent = client
        .ask_one(
            command::send_anon_request(&contact.public_key, &request),
            |r| matches!(r, Response::MessageSent { .. }),
        )
        .await?;
    let Response::MessageSent { tag, timeout_ms } = sent else {
        println!("refused: {sent:?}");
        return Ok(());
    };
    println!("sent, tag {}, timeout {timeout_ms} ms", hex(&tag));
    let wait = Duration::from_millis(u64::from(timeout_ms).max(5_000) * 2);
    let answer = tokio::time::timeout(wait, async {
        while let Some(push) = pushes.recv().await {
            match push {
                Response::BinaryResponse { tag: got, data } if got == tag => return Some(data),
                other => println!("push: {other:?}"),
            }
        }
        None
    })
    .await;
    match answer {
        Ok(Some(data)) => {
            let clock = data
                .get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
            let names = String::from_utf8_lossy(data.get(4..).unwrap_or_default()).into_owned();
            println!("clock {clock:?}, regions {names:?}");
        }
        Ok(None) => println!("the link closed"),
        Err(_) => println!("no answer in {} s", wait.as_secs()),
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
