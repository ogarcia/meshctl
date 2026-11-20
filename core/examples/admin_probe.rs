// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Asks a radio, through its client API, who it is and how its LoRa is set,
//! as admin requests, and prints what comes back: whether local admin answers,
//! and whether it hands out a session passkey. Read only: it changes nothing.
//!
//!     cargo run -p meshctl-core --example admin_probe -- 192.168.1.10 4403

use meshctl_core::link::{self, Transport};
use meshctl_proto::meshtastic::{
    AdminMessage, Data, FromRadio, MeshPacket, PortNum, ToRadio, admin_message, from_radio,
    mesh_packet, to_radio,
};
use prost::Message;
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or("usage: admin_probe <host> [port]")?;
    let port = args
        .next()
        .map_or(Ok(link::DEFAULT_TCP_PORT), |p| p.parse())?;
    let mut link = link::connect(&Transport::Tcp { host, port }).await?;

    let to_radio = |variant| ToRadio {
        payload_variant: Some(variant),
    };
    link.tx
        .send(to_radio(to_radio::PayloadVariant::WantConfigId(0x6d63_7461)).encode_to_vec())?;

    // Who the radio is, before anything can be addressed to it.
    let mut me = 0;
    while me == 0 {
        let payload = link.rx.recv().await.ok_or("link closed")?;
        if let Some(from_radio::PayloadVariant::MyInfo(info)) =
            FromRadio::decode(payload.as_slice())?.payload_variant
        {
            me = info.my_node_num;
        }
    }
    println!("radio is !{me:08x}");

    let ask = |id: u32, variant: admin_message::PayloadVariant| {
        let admin = AdminMessage {
            payload_variant: Some(variant),
            ..Default::default()
        };
        to_radio(to_radio::PayloadVariant::Packet(MeshPacket {
            to: me,
            id,
            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(Data {
                portnum: PortNum::AdminApp as i32,
                payload: admin.encode_to_vec(),
                want_response: true,
                ..Default::default()
            })),
            ..Default::default()
        }))
        .encode_to_vec()
    };
    link.tx.send(ask(
        0x1000_0001,
        admin_message::PayloadVariant::GetOwnerRequest(true),
    ))?;
    link.tx.send(ask(
        0x1000_0002,
        admin_message::PayloadVariant::GetConfigRequest(
            admin_message::ConfigType::LoraConfig as i32,
        ),
    ))?;

    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);
    loop {
        let payload = tokio::select! {
            payload = link.rx.recv() => payload.ok_or("link closed")?,
            () = &mut deadline => break,
        };
        let Some(from_radio::PayloadVariant::Packet(packet)) =
            FromRadio::decode(payload.as_slice())?.payload_variant
        else {
            continue;
        };
        let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant else {
            continue;
        };
        if data.portnum != PortNum::AdminApp as i32 {
            continue;
        }
        let admin = AdminMessage::decode(data.payload.as_slice())?;
        let what = format!("{:?}", admin.payload_variant);
        println!(
            "admin answer to {:#x} from !{:08x}: passkey {} bytes; {}",
            data.request_id,
            packet.from,
            admin.session_passkey.len(),
            &what[..what.len().min(160)],
        );
    }
    Ok(())
}
