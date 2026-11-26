// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Reads another node's settings over the mesh, as its admin, through a
//! radio's client API: what it has, then each part asked for, printed as it
//! answers or why not. Read only: it changes nothing, on the radio or the
//! node, and connects as the daemon would, without anything else it does.
//!
//!     cargo run -p meshctl-core --example remote_probe -- 192.168.1.10 4403 "Tiempo de Matanza" config.device module.mqtt

use meshctl_core::link::{self, Transport};
use meshctl_core::mesh::{LinkState, Settings, settings_request};
use meshctl_core::radio::{Mesh, Protocol};
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: remote_probe <host> <port> <node name or !id> [part…]";
    let host = args.next().ok_or(usage)?;
    let port = args
        .next()
        .map_or(Ok(link::DEFAULT_TCP_PORT), |p| p.parse())?;
    let wanted = args.next().ok_or(usage)?;
    let parts: Vec<String> = args.collect();

    let mesh = Mesh::default();
    mesh.spawn(Protocol::Meshtastic, Transport::Tcp { host, port });
    let started = std::time::Instant::now();
    while mesh.read(|s| s.link) != LinkState::Ready {
        if started.elapsed() > Duration::from_secs(60) {
            return Err("the radio did not sync within a minute".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let found = mesh.read(|state| {
        state.nodes.values().find_map(|n| {
            let user = n.user.as_ref()?;
            (user.long_name.eq_ignore_ascii_case(&wanted)
                || format!("!{:08x}", n.num).eq_ignore_ascii_case(&wanted))
            .then(|| (n.num, user.long_name.clone(), !user.public_key.is_empty()))
        })
    });
    let (num, name, keyed) = found.ok_or("no such node on the radio")?;
    println!("node !{num:08x} {name}, its key known: {keyed}");

    let mut settings = Settings::default();
    let mut asked = vec!["metadata".to_owned()];
    asked.extend(parts);
    for part in asked {
        let request = settings_request(&part).ok_or("not a part")?;
        let at = std::time::Instant::now();
        match mesh.ask_node(num, request).await {
            Ok(answer) => {
                settings.keep_answer(&answer);
                println!(
                    "{part}: answered in {:.1} s: {:?}",
                    at.elapsed().as_secs_f32(),
                    answer.payload_variant
                );
            }
            Err(error) => println!(
                "{part}: no, after {:.1} s: {error:?}",
                at.elapsed().as_secs_f32()
            ),
        }
    }
    Ok(())
}
