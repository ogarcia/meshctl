// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's clients, each on a TCP link of its own: what they send it,
//! and what it sends them.

use crate::radio::{Event, ME, NEIGHBOUR, Radio};
use meshctl_proto::frame::{self, FrameParser};
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, FromRadio, MeshPacket, PortNum, ToRadio, from_radio, mesh_packet,
    to_radio,
};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;

pub async fn send(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reply: from_radio::PayloadVariant,
) -> std::io::Result<()> {
    let message = FromRadio {
        id: 0,
        payload_variant: Some(reply),
    };
    let framed = frame::encode(&message.encode_to_vec()).expect("fits");
    writer.write_all(&framed).await
}

pub async fn serve(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    sender: broadcast::Sender<Event>,
) -> std::io::Result<()> {
    let mut events = sender.subscribe();
    let (mut reader, mut writer) = stream.into_split();
    let mut parser = FrameParser::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = tokio::select! {
            n = reader.read(&mut buf) => n?,
            event = events.recv() => {
                match event {
                    Ok(Event::Heard(packet)) => {
                        send(&mut writer, from_radio::PayloadVariant::Packet(packet)).await?;
                    }
                    Ok(Event::Drop) => return Ok(()),
                    Err(_) => {}
                }
                continue;
            }
        };
        if n == 0 {
            return Ok(());
        }
        for payload in parser.feed(&buf[..n]) {
            let Ok(message) = ToRadio::decode(payload.as_slice()) else {
                continue;
            };
            let replies: Vec<from_radio::PayloadVariant> = match message.payload_variant {
                Some(to_radio::PayloadVariant::WantConfigId(nonce)) => {
                    println!("configuration asked for");
                    radio.lock().unwrap().configuration(nonce)
                }
                Some(to_radio::PayloadVariant::Packet(packet)) => {
                    let Some(mesh_packet::PayloadVariant::Decoded(data)) =
                        packet.payload_variant.clone()
                    else {
                        continue;
                    };
                    if data.portnum != PortNum::AdminApp as i32 {
                        println!(
                            "packet to !{:08x} on port {}, not sent anywhere",
                            packet.to, data.portnum
                        );
                        // The neighbour answers what it is asked, in its own
                        // time, to every client, as heard over the air.
                        let answer = radio.lock().unwrap().ask(&packet, &data);
                        if let Some((delay, reply)) = answer {
                            println!("neighbour answers in {} ms", delay.as_millis());
                            let sender = sender.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(delay).await;
                                let _ = sender.send(Event::Heard(reply));
                            });
                        }
                        continue;
                    }
                    let Ok(admin) = AdminMessage::decode(data.payload.as_slice()) else {
                        continue;
                    };
                    // To any other node, over the air: nobody answers, and
                    // the radio gives up soon, as after its tries.
                    if packet.to != ME && packet.to != NEIGHBOUR {
                        println!("admin to !{:08x}: nobody answers", packet.to);
                        let gave_up = MeshPacket {
                            from: ME,
                            to: ME,
                            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                                portnum: PortNum::RoutingApp as i32,
                                payload: pb::Routing {
                                    variant: Some(pb::routing::Variant::ErrorReason(
                                        pb::routing::Error::MaxRetransmit as i32,
                                    )),
                                    ..Default::default()
                                }
                                .encode_to_vec(),
                                request_id: packet.id,
                                ..Default::default()
                            })),
                            ..Default::default()
                        };
                        let sender = sender.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            let _ = sender.send(Event::Heard(gave_up));
                        });
                        continue;
                    }
                    // To the neighbour, over the air: it answers in its time.
                    if packet.to == NEIGHBOUR {
                        let answer = radio.lock().unwrap().neighbour_admin(&packet, admin);
                        if let Some((delay, reply)) = answer {
                            let sender = sender.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(delay).await;
                                let _ = sender.send(Event::Heard(reply));
                            });
                        }
                        continue;
                    }
                    let answer = radio.lock().unwrap().admin(admin);
                    // What has its own answer gets it; what does not, but
                    // wants one, an acknowledgement, as the firmware's admin
                    // module gives.
                    let (portnum, payload) = match answer {
                        Some(answer) => (PortNum::AdminApp, answer.encode_to_vec()),
                        None if data.want_response => (
                            PortNum::RoutingApp,
                            pb::Routing {
                                variant: Some(pb::routing::Variant::ErrorReason(
                                    pb::routing::Error::None as i32,
                                )),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        ),
                        None => continue,
                    };
                    vec![from_radio::PayloadVariant::Packet(MeshPacket {
                        from: ME,
                        to: ME,
                        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                            portnum: portnum as i32,
                            payload,
                            request_id: packet.id,
                            ..Default::default()
                        })),
                        ..Default::default()
                    })]
                }
                _ => continue,
            };
            for reply in replies {
                send(&mut writer, reply).await?;
            }
        }
    }
}
