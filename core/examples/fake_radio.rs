// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend Meshtastic radio on TCP, for working on meshctl without one, or
//! without risking one: it sends every part of the configuration and every
//! module these protobufs know, answers admin requests with a passkey, and
//! takes in what it is told to change, so that reading it back shows it.
//! It transmits nothing, anywhere.
//!
//!     cargo run -p meshctl-core --example fake_radio -- [port]

use meshctl_proto::descriptors;
use meshctl_proto::frame::{self, FrameParser};
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, FromRadio, MeshPacket, PortNum, ToRadio, admin_message, config,
    from_radio, mesh_packet, module_config, to_radio,
};
use prost::Message;
use prost_reflect::{DynamicMessage, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ME: u32 = 0x1234_abcd;
const PASSKEY: [u8; 8] = [7; 8];

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
            V::Metadata(pb::DeviceMetadata {
                firmware_version: "0.0.0.fake".into(),
                hw_model: pb::HardwareModel::HeltecV3 as i32,
                ..Default::default()
            }),
            V::NodeInfo(pb::NodeInfo {
                num: ME,
                user: Some(self.owner.clone()),
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

async fn serve(stream: TcpStream, radio: Arc<Mutex<Radio>>) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let mut parser = FrameParser::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = reader.read(&mut buf).await?;
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
                let message = FromRadio {
                    id: 0,
                    payload_variant: Some(reply),
                };
                let framed = frame::encode(&message.encode_to_vec()).expect("fits");
                writer.write_all(&framed).await?;
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args().nth(1).map_or(Ok(4403), |p| p.parse())?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let radio = Arc::new(Mutex::new(Radio::new()));
    println!("fake radio !{ME:08x} on port {port}");
    loop {
        let (stream, address) = listener.accept().await?;
        println!("client {address}");
        let radio = Arc::clone(&radio);
        tokio::spawn(async move {
            if let Err(error) = serve(stream, radio).await {
                println!("client {address} gone: {error}");
            }
        });
    }
}
