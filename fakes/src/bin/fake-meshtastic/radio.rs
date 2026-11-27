// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio: what it is set to, as it would keep it, and what it hands
//! its clients.

use crate::neighbour::{Answers, NeighbourSettings, neighbour};
use meshctl_fakes::now;
use meshctl_proto::descriptors;
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, MeshPacket, admin_message, config, from_radio, module_config,
};
use prost::Message;
use prost_reflect::{DynamicMessage, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

pub const ME: u32 = 0x1234_abcd;
pub const PASSKEY: [u8; 8] = [7; 8];
/// What the neighbour gives out to change its settings.
pub const NEIGHBOUR_PASSKEY: [u8; 8] = [9; 8];
pub const BROADCAST: u32 = 0xffff_ffff;
/// The node of its mesh heard directly, who `hear` has speak.
pub const NEIGHBOUR: u32 = 0x0bad_cafe;
/// A repeater further away, which takes no messages.
pub const REPEATER: u32 = 0x0dec_0de5;

/// What the control port has happen to every client.
#[derive(Debug, Clone)]
pub enum Event {
    /// Something heard, passed on.
    Heard(MeshPacket),
    /// The link drops.
    Drop,
}

/// What the radio is set to, as it would keep it.
pub struct Radio {
    pub owner: pb::User,
    pub configs: BTreeMap<String, pb::Config>,
    pub modules: BTreeMap<String, pb::ModuleConfig>,
    pub channels: BTreeMap<i32, pb::Channel>,
    /// How the neighbour answers what it is asked.
    pub answers: Answers,
    /// Whether the neighbour keeps from answering too often, as nodes do.
    pub limits: bool,
    /// When the neighbour last answered, by port.
    pub answered: BTreeMap<i32, Instant>,
    /// When a client last sent on a paced port, which the radio takes no
    /// more often than the firmware does.
    pub paced: BTreeMap<i32, Instant>,
    /// The nodes marked favourites.
    pub favorites: std::collections::BTreeSet<u32>,
    /// The nodes a client had it forget: left out of its node list.
    pub removed: std::collections::BTreeSet<u32>,
    /// The last text a client sent, for the mesh's examples to answer.
    pub last_text: Option<u32>,
    /// Where it was told it is, fixed, as its own node says.
    pub fixed: Option<pb::Position>,
    /// Contacts a client added, as nodes it keeps.
    pub contacts: BTreeMap<u32, pb::User>,
    /// Whether the neighbour takes the radio's key as an admin key.
    pub admin_key: bool,
    /// The neighbour's own settings, once asked as its admin.
    pub neighbour: Option<NeighbourSettings>,
}

/// A packet's id from one of the counters, each with its own top byte: the
/// rest moved on by when this run started, so that a run does not use the
/// ids of the one before, which a client would take for the same packets
/// heard again.
pub fn fresh_id(counter: &AtomicU32) -> u32 {
    static SALT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let salt = *SALT.get_or_init(|| now().wrapping_mul(97) & 0x00ff_ffff);
    let next = counter.fetch_add(1, Ordering::Relaxed);
    (next & 0xff00_0000) | (next.wrapping_add(salt) & 0x00ff_ffff)
}

/// The name of the one part a `Config` or `ModuleConfig` carries.
pub fn part_name<M: Message>(carrier: &str, message: &M) -> Option<String> {
    let descriptor = descriptors().get_message_by_name(carrier)?;
    let dynamic = DynamicMessage::decode(descriptor, message.encode_to_vec().as_slice()).ok()?;
    dynamic.fields().next().map(|(f, _)| f.name().to_owned())
}

/// One of each part a carrier can hold, empty: every section the firmware
/// these protobufs describe would send.
pub fn every_part<M: Message + Default>(carrier: &str) -> BTreeMap<String, M> {
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
    pub fn new() -> Self {
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
            answers: Answers::Always,
            limits: true,
            answered: BTreeMap::new(),
            paced: BTreeMap::new(),
            favorites: std::collections::BTreeSet::new(),
            removed: std::collections::BTreeSet::new(),
            last_text: None,
            fixed: None,
            contacts: BTreeMap::new(),
            admin_key: true,
            neighbour: None,
        }
    }

    /// Everything a client asking for its configuration gets.
    pub fn configuration(&self, nonce: u32) -> Vec<from_radio::PayloadVariant> {
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
                position: self.fixed,
                ..Default::default()
            }),
            V::NodeInfo(pb::NodeInfo {
                num: NEIGHBOUR,
                user: Some(neighbour()),
                hops_away: Some(0),
                is_favorite: self.favorites.contains(&NEIGHBOUR),
                ..Default::default()
            }),
            // A repeater two hops away, which tells nobody reads messages
            // sent to it.
            V::NodeInfo(pb::NodeInfo {
                num: REPEATER,
                user: Some(pb::User {
                    id: format!("!{REPEATER:08x}"),
                    long_name: "Fake repeater".into(),
                    short_name: "RPT".into(),
                    hw_model: pb::HardwareModel::Rak4631 as i32,
                    role: config::device_config::Role::Router as i32,
                    public_key: (33..=64).collect(),
                    is_unmessagable: Some(true),
                    ..Default::default()
                }),
                // On a hill near the neighbour, to be on the map.
                position: Some(pb::Position {
                    latitude_i: Some(404_310_000),
                    longitude_i: Some(-37_150_000),
                    altitude: Some(780),
                    location_source: pb::position::LocSource::LocManual as i32,
                    precision_bits: 32,
                    ..Default::default()
                }),
                hops_away: Some(2),
                last_heard: now() - 600,
                is_favorite: self.favorites.contains(&REPEATER),
                ..Default::default()
            }),
        ];
        // The contacts added to it, as nodes not heard yet.
        out.extend(self.contacts.iter().map(|(num, user)| {
            V::NodeInfo(pb::NodeInfo {
                num: *num,
                user: Some(user.clone()),
                is_favorite: self.favorites.contains(num),
                ..Default::default()
            })
        }));
        // Those it was told to forget, out, until it is restarted.
        out.retain(|v| !matches!(v, V::NodeInfo(n) if self.removed.contains(&n.num)));
        out.extend(self.channels.values().cloned().map(V::Channel));
        out.extend(self.configs.values().cloned().map(V::Config));
        out.extend(self.modules.values().cloned().map(V::ModuleConfig));
        out.push(V::ConfigCompleteId(nonce));
        out
    }

    /// Has its position fixed, or no longer, as the firmware does along
    /// with setting or removing one.
    pub fn fix(&mut self, fixed: bool) {
        let entry = self
            .configs
            .entry("position".to_owned())
            .or_insert_with(|| pb::Config {
                payload_variant: Some(config::PayloadVariant::Position(
                    config::PositionConfig::default(),
                )),
            });
        if let Some(config::PayloadVariant::Position(position)) = &mut entry.payload_variant {
            position.fixed_position = fixed;
        }
    }

    /// Takes in an admin message; returns the answer, if it asks for one.
    pub fn admin(&mut self, admin: AdminMessage) -> Option<AdminMessage> {
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
            A::SetFixedPosition(position) => {
                println!(
                    "fixed position: {:?}, {:?}, {:?} m",
                    position.latitude_i, position.longitude_i, position.altitude
                );
                self.fixed = Some(position);
                self.fix(true);
                None
            }
            A::RemoveFixedPosition(_) => {
                println!("fixed position removed");
                self.fixed = None;
                self.fix(false);
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
            A::SetFavoriteNode(num) => {
                println!("favourite: !{num:08x}");
                self.favorites.insert(num);
                None
            }
            A::AddContact(contact) => {
                println!("contact: !{:08x}", contact.node_num);
                if let Some(user) = contact.user {
                    self.removed.remove(&contact.node_num);
                    self.contacts.insert(contact.node_num, user);
                }
                None
            }
            A::RemoveByNodenum(num) => {
                println!("forgotten: !{num:08x}");
                self.removed.insert(num);
                None
            }
            A::RebootSeconds(seconds) => {
                println!("reboots in {seconds} s (a real radio would)");
                None
            }
            A::ShutdownSeconds(seconds) => {
                println!("shuts down in {seconds} s (a real radio would)");
                None
            }
            A::NodedbReset(_) => {
                println!("forgets its nodes (a real radio would)");
                None
            }
            A::FactoryResetConfig(_) => {
                println!("goes back to its factory settings (a real radio would)");
                None
            }
            A::RemoveFavoriteNode(num) => {
                println!("no longer favourite: !{num:08x}");
                self.favorites.remove(&num);
                None
            }
            other => {
                println!("admin, ignored: {other:?}");
                None
            }
        }
    }
}
