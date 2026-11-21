// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend Meshtastic radio on TCP, for working on meshctl without one, or
//! without risking one: it sends every part of the configuration and every
//! module these protobufs know, answers admin requests with a passkey, and
//! takes in what it is told to change, so that reading it back shows it.
//! It transmits nothing, anywhere.
//!
//!     cargo run -p meshctl-core --example fake_radio -- [port] [control port]
//!
//! With a control port, it listens there, on this machine only, for lines
//! that play what another app or the mesh would do to it:
//!
//! - `hear <slot> <text>`: a node of its mesh sends that on a channel.
//! - `twin <from> <to>`: the channel in one slot is copied into another, the
//!   very same channel twice, as another app may leave it.
//! - `off <slot>`: the channel in a slot is switched off.
//! - `drop`: the link to every client drops, which reads it all anew.

use meshctl_proto::descriptors;
use meshctl_proto::frame::{self, FrameParser};
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, FromRadio, MeshPacket, PortNum, ToRadio, admin_message, config,
    from_radio, mesh_packet, module_config, to_radio,
};
use prost::Message;
use prost_reflect::{DynamicMessage, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

const ME: u32 = 0x1234_abcd;
const PASSKEY: [u8; 8] = [7; 8];
const BROADCAST: u32 = 0xffff_ffff;
/// The one other node of its mesh, who `hear` has speak.
const NEIGHBOUR: u32 = 0x0bad_cafe;

/// What the control port has happen to every client.
#[derive(Debug, Clone)]
enum Event {
    /// Something heard, passed on.
    Heard(MeshPacket),
    /// The link drops.
    Drop,
}

/// What the radio is set to, as it would keep it.
struct Radio {
    owner: pb::User,
    configs: BTreeMap<String, pb::Config>,
    modules: BTreeMap<String, pb::ModuleConfig>,
    channels: BTreeMap<i32, pb::Channel>,
}

/// The name of the one part a `Config` or `ModuleConfig` carries.
fn part_name<M: Message>(carrier: &str, message: &M) -> Option<String> {
    let descriptor = descriptors().get_message_by_name(carrier)?;
    let dynamic = DynamicMessage::decode(descriptor, message.encode_to_vec().as_slice()).ok()?;
    dynamic.fields().next().map(|(f, _)| f.name().to_owned())
}

/// One of each part a carrier can hold, empty: every section the firmware
/// these protobufs describe would send.
fn every_part<M: Message + Default>(carrier: &str) -> BTreeMap<String, M> {
    let descriptor = descriptors()
        .get_message_by_name(carrier)
        .expect("described");
    descriptor
        .oneofs()
        .flat_map(|oneof| oneof.fields().collect::<Vec<_>>())
        .filter_map(|field| {
            let prost_reflect::Kind::Message(part) = field.kind() else {
                return None;
            };
            let mut whole = DynamicMessage::new(descriptor.clone());
            whole.set_field(&field, Value::Message(DynamicMessage::new(part)));
            let message = M::decode(whole.encode_to_vec().as_slice()).ok()?;
            Some((field.name().to_owned(), message))
        })
        .collect()
}

impl Radio {
    fn new() -> Self {
        let mut configs: BTreeMap<String, pb::Config> = every_part("meshtastic.Config");
        configs.insert(
            "device".into(),
            pb::Config {
                payload_variant: Some(config::PayloadVariant::Device(config::DeviceConfig {
                    role: config::device_config::Role::Client as i32,
                    node_info_broadcast_secs: 10_800,
                    ..Default::default()
                })),
            },
        );
        configs.insert(
            "lora".into(),
            pb::Config {
                payload_variant: Some(config::PayloadVariant::Lora(config::LoRaConfig {
                    use_preset: true,
                    region: config::lo_ra_config::RegionCode::Eu868 as i32,
                    modem_preset: config::lo_ra_config::ModemPreset::LongFast as i32,
                    hop_limit: 3,
                    tx_enabled: true,
                    ..Default::default()
                })),
            },
        );
        configs.insert(
            "network".into(),
            pb::Config {
                payload_variant: Some(config::PayloadVariant::Network(config::NetworkConfig {
                    wifi_enabled: true,
                    wifi_ssid: "lab".into(),
                    wifi_psk: "not-for-your-eyes".into(),
                    ..Default::default()
                })),
            },
        );
        let mut modules: BTreeMap<String, pb::ModuleConfig> = every_part("meshtastic.ModuleConfig");
        modules.insert(
            "mqtt".into(),
            pb::ModuleConfig {
                payload_variant: Some(module_config::PayloadVariant::Mqtt(
                    module_config::MqttConfig {
                        address: "mqtt.lab".into(),
                        username: "meshctl".into(),
                        password: "not-for-your-eyes".into(),
                        ..Default::default()
                    },
                )),
            },
        );
        let channels = (0..8)
            .map(|index| {
                let (role, name, psk) = match index {
                    0 => (pb::channel::Role::Primary, "", vec![1]),
                    1 => (pb::channel::Role::Secondary, "Lab", vec![9; 32]),
                    _ => (pb::channel::Role::Disabled, "", vec![]),
                };
                let channel = pb::Channel {
                    index,
                    role: role as i32,
                    settings: Some(pb::ChannelSettings {
                        name: name.into(),
                        psk,
                        ..Default::default()
                    }),
                };
                (index, channel)
            })
            .collect();
        Self {
            owner: pb::User {
                id: format!("!{ME:08x}"),
                long_name: "Fake radio".into(),
                short_name: "FAKE".into(),
                hw_model: pb::HardwareModel::HeltecV3 as i32,
                ..Default::default()
            },
            configs,
            modules,
            channels,
        }
    }

    /// Everything a client asking for its configuration gets.
    fn configuration(&self, nonce: u32) -> Vec<from_radio::PayloadVariant> {
        use from_radio::PayloadVariant as V;
        let mut out = vec![
            V::MyInfo(pb::MyNodeInfo {
                my_node_num: ME,
                ..Default::default()
            }),
            // As a Heltec V3 says it: Wi-Fi and Bluetooth, no Ethernet.
            V::Metadata(pb::DeviceMetadata {
                firmware_version: "0.0.0.fake".into(),
                hw_model: pb::HardwareModel::HeltecV3 as i32,
                has_wifi: true,
                has_bluetooth: true,
                has_ethernet: false,
                ..Default::default()
            }),
            V::NodeInfo(pb::NodeInfo {
                num: ME,
                user: Some(self.owner.clone()),
                ..Default::default()
            }),
            V::NodeInfo(pb::NodeInfo {
                num: NEIGHBOUR,
                user: Some(pb::User {
                    id: format!("!{NEIGHBOUR:08x}"),
                    long_name: "Fake neighbour".into(),
                    short_name: "NBR".into(),
                    ..Default::default()
                }),
                hops_away: Some(0),
                ..Default::default()
            }),
        ];
        out.extend(self.channels.values().cloned().map(V::Channel));
        out.extend(self.configs.values().cloned().map(V::Config));
        out.extend(self.modules.values().cloned().map(V::ModuleConfig));
        out.push(V::ConfigCompleteId(nonce));
        out
    }

    /// Takes in an admin message; returns the answer, if it asks for one.
    fn admin(&mut self, admin: AdminMessage) -> Option<AdminMessage> {
        use admin_message::PayloadVariant as A;
        let answer = |variant| {
            Some(AdminMessage {
                session_passkey: PASSKEY.to_vec(),
                payload_variant: Some(variant),
            })
        };
        let changes = !matches!(admin.payload_variant, Some(A::GetOwnerRequest(_)));
        if changes && admin.session_passkey != PASSKEY {
            println!("refused: wrong passkey");
            return None;
        }
        match admin.payload_variant? {
            A::GetOwnerRequest(_) => answer(A::GetOwnerResponse(self.owner.clone())),
            A::SetOwner(owner) => {
                println!("owner: {} / {}", owner.long_name, owner.short_name);
                self.owner = owner;
                None
            }
            A::SetConfig(config) => {
                let name = part_name("meshtastic.Config", &config)?;
                println!("config.{name}: {config:?}");
                self.configs.insert(name, config);
                None
            }
            A::SetModuleConfig(module) => {
                let name = part_name("meshtastic.ModuleConfig", &module)?;
                println!("module.{name}: {module:?}");
                self.modules.insert(name, module);
                None
            }
            A::SetChannel(channel) => {
                println!(
                    "channel {}: {:?}",
                    channel.index,
                    channel.settings.as_ref().map(|s| &s.name)
                );
                self.channels.insert(channel.index, channel);
                None
            }
            A::BeginEditSettings(_) => {
                println!("begin edit");
                None
            }
            A::CommitEditSettings(_) => {
                println!("commit edit (a real radio might reboot now)");
                None
            }
            other => {
                println!("admin, ignored: {other:?}");
                None
            }
        }
    }
}

async fn send(
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

async fn serve(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    mut events: broadcast::Receiver<Event>,
) -> std::io::Result<()> {
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
                    let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant
                    else {
                        continue;
                    };
                    if data.portnum != PortNum::AdminApp as i32 {
                        println!(
                            "packet to !{:08x} on port {}, not sent anywhere",
                            packet.to, data.portnum
                        );
                        continue;
                    }
                    let Ok(admin) = AdminMessage::decode(data.payload.as_slice()) else {
                        continue;
                    };
                    let answer = radio.lock().unwrap().admin(admin);
                    answer
                        .map(|answer| {
                            from_radio::PayloadVariant::Packet(MeshPacket {
                                from: ME,
                                to: ME,
                                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(
                                    pb::Data {
                                        portnum: PortNum::AdminApp as i32,
                                        payload: answer.encode_to_vec(),
                                        request_id: packet.id,
                                        ..Default::default()
                                    },
                                )),
                                ..Default::default()
                            })
                        })
                        .into_iter()
                        .collect()
                }
                _ => continue,
            };
            for reply in replies {
                send(&mut writer, reply).await?;
            }
        }
    }
}

/// Does what a line of the control port says; what to answer it.
fn control(line: &str, radio: &Mutex<Radio>, events: &broadcast::Sender<Event>) -> String {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0x5000_0000);
    let mut words = line.trim().splitn(3, ' ');
    let slot = |word: Option<&str>| {
        word.and_then(|w| w.parse::<i32>().ok())
            .filter(|s| (0..8).contains(s))
    };
    match words.next() {
        Some("hear") => {
            let (Some(channel), Some(text)) = (slot(words.next()), words.next()) else {
                return "usage: hear <slot> <text>".into();
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as u32);
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                channel: channel as u32,
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                rx_time: Some(now),
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::TextMessageApp as i32,
                    payload: text.as_bytes().to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("heard on slot {channel}")
        }
        Some("twin") => {
            let (Some(from), Some(to)) = (slot(words.next()), slot(words.next())) else {
                return "usage: twin <from> <to>".into();
            };
            let mut radio = radio.lock().unwrap();
            let Some(mut channel) = radio.channels.get(&from).cloned() else {
                return "no such channel".into();
            };
            channel.index = to;
            channel.role = if to == 0 {
                pb::channel::Role::Primary
            } else {
                pb::channel::Role::Secondary
            } as i32;
            radio.channels.insert(to, channel);
            format!("slot {to} is slot {from} again; drop to have it read")
        }
        Some("off") => {
            let Some(index) = slot(words.next()).filter(|s| *s != 0) else {
                return "usage: off <slot>, not 0".into();
            };
            radio.lock().unwrap().channels.insert(
                index,
                pb::Channel {
                    index,
                    role: pb::channel::Role::Disabled as i32,
                    settings: Some(pb::ChannelSettings::default()),
                },
            );
            format!("slot {index} off; drop to have it read")
        }
        Some("drop") => {
            let _ = events.send(Event::Drop);
            "dropped".into()
        }
        _ => "hear <slot> <text> | twin <from> <to> | off <slot> | drop".into(),
    }
}

async fn controlled(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    events: broadcast::Sender<Event>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let answer = control(&line, &radio, &events);
        println!("control: {line} -> {answer}");
        writer.write_all(format!("{answer}\n").as_bytes()).await?;
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().map_or(Ok(4403), |p| p.parse())?;
    let control_port: Option<u16> = args.next().map(|p| p.parse()).transpose()?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let radio = Arc::new(Mutex::new(Radio::new()));
    let (events, _) = broadcast::channel(64);
    println!("fake radio !{ME:08x} on port {port}");
    if let Some(control_port) = control_port {
        let control = TcpListener::bind(("127.0.0.1", control_port)).await?;
        println!("control on 127.0.0.1:{control_port}");
        let radio = Arc::clone(&radio);
        let events = events.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = control.accept().await {
                let (radio, events) = (Arc::clone(&radio), events.clone());
                tokio::spawn(async move {
                    let _ = controlled(stream, radio, events).await;
                });
            }
        });
    }
    loop {
        let (stream, address) = listener.accept().await?;
        println!("client {address}");
        let radio = Arc::clone(&radio);
        let events = events.subscribe();
        tokio::spawn(async move {
            if let Err(error) = serve(stream, radio, events).await {
                println!("client {address} gone: {error}");
            }
        });
    }
}
