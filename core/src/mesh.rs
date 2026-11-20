// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl knows about the radio and the mesh around it, and how each
//! message from the radio changes that.
//!
//! [`MeshState::apply`] is pure: it takes a `FromRadio` and the time, changes
//! the state and says what changed. Everything about connections and timing is
//! in [`crate::radio`].

use meshctl_proto::meshtastic::{
    self as pb, HardwareModel, PortNum, config, from_radio, mesh_packet, telemetry,
};
use prost::Message;
use std::collections::BTreeMap;

/// How the link to the radio is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    /// Not connected, and waiting to try again.
    #[default]
    Disconnected,
    /// Trying to connect.
    Connecting,
    /// Connected, and receiving the radio's configuration and node list.
    Syncing,
    /// Connected and up to date.
    Ready,
}

/// The radio meshctl is connected to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalNode {
    pub num: u32,
    pub firmware_version: Option<String>,
    pub hw_model: Option<String>,
    pub role: Option<String>,
    pub region: Option<String>,
    pub modem_preset: Option<String>,
}

/// A channel configured on the radio. Its key is deliberately not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub index: u32,
    /// Empty for a primary channel that takes the modem preset's name.
    pub name: String,
    pub role: String,
}

/// Who a node says it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct User {
    pub long_name: String,
    pub short_name: String,
    pub hw_model: String,
    pub role: String,
}

/// Where a node says it is, in degrees and metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<i32>,
}

/// A node of the mesh, as heard from the radio.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub num: u32,
    pub user: Option<User>,
    pub position: Option<Position>,
    /// When it was last heard, in seconds since the epoch.
    pub last_heard: Option<u32>,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hops_away: Option<u32>,
    pub via_mqtt: bool,
    pub favorite: bool,
    pub battery_level: Option<u32>,
    pub voltage: Option<f32>,
}

impl Node {
    fn new(num: u32) -> Self {
        Self {
            num,
            user: None,
            position: None,
            last_heard: None,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
        }
    }

    /// The node's id, the way Meshtastic writes it: `!` and eight hex digits.
    pub fn id(&self) -> String {
        format!("!{:08x}", self.num)
    }
}

/// What changed, for whoever is following along.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Link(LinkState),
    /// The radio finished sending its configuration and node list: anything
    /// read before is out of date.
    Synced,
    Node(Node),
}

/// Everything known about the radio and the mesh.
#[derive(Debug, Clone, Default)]
pub struct MeshState {
    pub link: LinkState,
    pub local: Option<LocalNode>,
    pub channels: BTreeMap<u32, Channel>,
    pub nodes: BTreeMap<u32, Node>,
}

impl MeshState {
    /// Takes in a message from the radio received at `now` (seconds since the
    /// epoch) and returns what changed.
    ///
    /// While syncing, nodes change without an event each: the radio sends its
    /// whole list at once, and [`Event::Synced`] at the end says it all.
    pub fn apply(&mut self, message: pb::FromRadio, now: u32) -> Vec<Event> {
        use from_radio::PayloadVariant as V;

        let quiet = self.link == LinkState::Syncing;
        let changed = match message.payload_variant {
            Some(V::MyInfo(info)) => {
                // Another radio on the same address: what was known is theirs.
                if self
                    .local
                    .as_ref()
                    .is_some_and(|l| l.num != info.my_node_num)
                {
                    self.nodes.clear();
                    self.channels.clear();
                }
                self.local.get_or_insert_with(LocalNode::default).num = info.my_node_num;
                None
            }
            Some(V::Metadata(metadata)) => {
                let local = self.local.get_or_insert_with(LocalNode::default);
                local.firmware_version = Some(metadata.firmware_version);
                local.hw_model = Some(hw_model_name(metadata.hw_model));
                None
            }
            Some(V::Config(config)) => {
                let local = self.local.get_or_insert_with(LocalNode::default);
                match config.payload_variant {
                    Some(config::PayloadVariant::Device(device)) => {
                        local.role = Some(role_name(device.role));
                    }
                    Some(config::PayloadVariant::Lora(lora)) => {
                        local.region = Some(enum_name(lora.region, |v| {
                            config::lo_ra_config::RegionCode::try_from(v)
                                .ok()
                                .map(|c| c.as_str_name())
                        }));
                        local.modem_preset = lora.use_preset.then(|| {
                            enum_name(lora.modem_preset, |v| {
                                config::lo_ra_config::ModemPreset::try_from(v)
                                    .ok()
                                    .map(|p| p.as_str_name())
                            })
                        });
                    }
                    _ => {}
                }
                None
            }
            Some(V::Channel(channel)) => {
                let index = u32::try_from(channel.index).unwrap_or_default();
                let role = enum_name(channel.role, |v| {
                    pb::channel::Role::try_from(v).ok().map(|r| r.as_str_name())
                });
                self.channels.insert(
                    index,
                    Channel {
                        index,
                        name: channel.settings.map(|s| s.name).unwrap_or_default(),
                        role,
                    },
                );
                None
            }
            Some(V::NodeInfo(info)) => Some(self.node_info(info)),
            Some(V::Packet(packet)) => Some(self.packet(packet, now)),
            _ => None,
        };

        match changed {
            Some(node) if !quiet => vec![Event::Node(node)],
            _ => Vec::new(),
        }
    }

    fn node_info(&mut self, info: pb::NodeInfo) -> Node {
        let node = self
            .nodes
            .entry(info.num)
            .or_insert_with(|| Node::new(info.num));
        if let Some(user) = info.user {
            node.user = Some(user_from(user));
        }
        if let Some(position) = info.position.and_then(position_from) {
            node.position = Some(position);
        }
        if let Some(metrics) = info.device_metrics {
            node.battery_level = metrics.battery_level.or(node.battery_level);
            node.voltage = metrics.voltage.or(node.voltage);
        }
        node.last_heard = (info.last_heard > 0).then_some(info.last_heard);
        node.snr = (info.snr != 0.0).then_some(info.snr);
        node.hops_away = info.hops_away;
        node.via_mqtt = info.via_mqtt;
        node.favorite = info.is_favorite;
        node.clone()
    }

    fn packet(&mut self, packet: pb::MeshPacket, now: u32) -> Node {
        let ours = self.local.as_ref().is_some_and(|l| l.num == packet.from);
        let node = self
            .nodes
            .entry(packet.from)
            .or_insert_with(|| Node::new(packet.from));

        node.last_heard = Some(packet.rx_time.filter(|t| *t > 0).unwrap_or(now));
        node.via_mqtt = packet.via_mqtt;
        // Over MQTT there is no radio reception to speak of, nor hops counted,
        // and what the radio itself sends was never received at all.
        if !packet.via_mqtt && !ours {
            node.snr = Some(packet.rx_snr);
            node.rssi = packet.rx_rssi;
            if packet.hop_start > 0 {
                node.hops_away = Some(packet.hop_start.saturating_sub(packet.hop_limit));
            }
        }

        if let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant {
            match PortNum::try_from(data.portnum) {
                Ok(PortNum::NodeinfoApp) => {
                    if let Ok(user) = pb::User::decode(data.payload.as_slice()) {
                        node.user = Some(user_from(user));
                    }
                }
                Ok(PortNum::PositionApp) => {
                    if let Some(position) = pb::Position::decode(data.payload.as_slice())
                        .ok()
                        .and_then(position_from)
                    {
                        node.position = Some(position);
                    }
                }
                Ok(PortNum::TelemetryApp) => {
                    if let Ok(pb::Telemetry {
                        variant: Some(telemetry::Variant::DeviceMetrics(metrics)),
                        ..
                    }) = pb::Telemetry::decode(data.payload.as_slice())
                    {
                        node.battery_level = metrics.battery_level.or(node.battery_level);
                        node.voltage = metrics.voltage.or(node.voltage);
                    }
                }
                _ => {}
            }
        }

        node.clone()
    }
}

fn user_from(user: pb::User) -> User {
    User {
        long_name: user.long_name,
        short_name: user.short_name,
        hw_model: hw_model_name(user.hw_model),
        role: role_name(user.role),
    }
}

/// A position, if it says where. Zero for both is what an unset one reads as.
fn position_from(position: pb::Position) -> Option<Position> {
    let (latitude, longitude) = (position.latitude_i?, position.longitude_i?);
    if latitude == 0 && longitude == 0 {
        return None;
    }
    Some(Position {
        latitude: f64::from(latitude) * 1e-7,
        longitude: f64::from(longitude) * 1e-7,
        altitude: position.altitude,
    })
}

/// The protobuf name of an enum value, or the number when these protobufs are
/// older than the firmware that sent it.
fn enum_name(value: i32, name: impl Fn(i32) -> Option<&'static str>) -> String {
    name(value).map_or_else(|| value.to_string(), str::to_owned)
}

fn hw_model_name(value: i32) -> String {
    enum_name(value, |v| {
        HardwareModel::try_from(v).ok().map(|m| m.as_str_name())
    })
}

fn role_name(value: i32) -> String {
    enum_name(value, |v| {
        config::device_config::Role::try_from(v)
            .ok()
            .map(|r| r.as_str_name())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use from_radio::PayloadVariant as V;

    fn from_radio(variant: V) -> pb::FromRadio {
        pb::FromRadio {
            id: 0,
            payload_variant: Some(variant),
        }
    }

    fn packet(from: u32, port: PortNum, payload: Vec<u8>) -> pb::MeshPacket {
        pb::MeshPacket {
            from,
            to: 0xffff_ffff,
            rx_snr: 6.5,
            rx_rssi: Some(-90),
            hop_start: 5,
            hop_limit: 3,
            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                portnum: port as i32,
                payload,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn ready() -> MeshState {
        MeshState {
            link: LinkState::Ready,
            ..Default::default()
        }
    }

    #[test]
    fn a_packet_from_a_stranger_makes_a_node() {
        let mut state = ready();
        let events = state.apply(
            from_radio(V::Packet(packet(
                0x1234_5678,
                PortNum::TextMessageApp,
                vec![],
            ))),
            1000,
        );

        let node = &state.nodes[&0x1234_5678];
        assert_eq!(node.id(), "!12345678");
        assert_eq!(node.last_heard, Some(1000));
        assert_eq!(node.snr, Some(6.5));
        assert_eq!(node.rssi, Some(-90));
        assert_eq!(node.hops_away, Some(2));
        assert_eq!(events, vec![Event::Node(node.clone())]);
    }

    #[test]
    fn nodeinfo_says_who_it_is() {
        let mut state = ready();
        let user = pb::User {
            long_name: "Faro".into(),
            short_name: "FARO".into(),
            hw_model: HardwareModel::HeltecV3 as i32,
            role: config::device_config::Role::Router as i32,
            ..Default::default()
        };
        state.apply(
            from_radio(V::Packet(packet(
                7,
                PortNum::NodeinfoApp,
                user.encode_to_vec(),
            ))),
            1,
        );
        let user = state.nodes[&7].user.clone().unwrap();
        assert_eq!(user.long_name, "Faro");
        assert_eq!(user.hw_model, "HELTEC_V3");
        assert_eq!(user.role, "ROUTER");
    }

    #[test]
    fn position_and_battery_are_kept() {
        let mut state = ready();
        let position = pb::Position {
            latitude_i: Some(433_623_000),
            longitude_i: Some(-84_115_000),
            ..Default::default()
        };
        state.apply(
            from_radio(V::Packet(packet(
                7,
                PortNum::PositionApp,
                position.encode_to_vec(),
            ))),
            1,
        );
        let telemetry = pb::Telemetry {
            variant: Some(telemetry::Variant::DeviceMetrics(pb::DeviceMetrics {
                battery_level: Some(87),
                ..Default::default()
            })),
            ..Default::default()
        };
        state.apply(
            from_radio(V::Packet(packet(
                7,
                PortNum::TelemetryApp,
                telemetry.encode_to_vec(),
            ))),
            2,
        );

        let node = &state.nodes[&7];
        let position = node.position.unwrap();
        assert!((position.latitude - 43.3623).abs() < 1e-9);
        assert!((position.longitude + 8.4115).abs() < 1e-9);
        assert_eq!(node.battery_level, Some(87));
    }

    #[test]
    fn mqtt_does_not_count_as_reception() {
        let mut state = ready();
        let mut via = packet(7, PortNum::TextMessageApp, vec![]);
        via.via_mqtt = true;
        state.apply(from_radio(V::Packet(via)), 1);
        let node = &state.nodes[&7];
        assert!(node.via_mqtt);
        assert_eq!(node.snr, None);
        assert_eq!(node.hops_away, None);
    }

    #[test]
    fn the_radio_does_not_hear_itself() {
        let mut state = ready();
        state.local = Some(LocalNode {
            num: 7,
            ..Default::default()
        });
        state.apply(
            from_radio(V::Packet(packet(7, PortNum::TelemetryApp, vec![]))),
            5,
        );
        let node = &state.nodes[&7];
        assert_eq!(node.last_heard, Some(5));
        assert_eq!(node.snr, None);
        assert_eq!(node.rssi, None);
    }

    #[test]
    fn syncing_is_quiet() {
        let mut state = MeshState {
            link: LinkState::Syncing,
            ..Default::default()
        };
        let events = state.apply(
            from_radio(V::NodeInfo(pb::NodeInfo {
                num: 7,
                last_heard: 10,
                ..Default::default()
            })),
            1,
        );
        assert!(events.is_empty());
        assert_eq!(state.nodes[&7].last_heard, Some(10));
    }

    #[test]
    fn another_radio_forgets_the_last_ones_mesh() {
        let mut state = ready();
        state.apply(
            from_radio(V::MyInfo(pb::MyNodeInfo {
                my_node_num: 1,
                ..Default::default()
            })),
            1,
        );
        state.apply(
            from_radio(V::Packet(packet(7, PortNum::TextMessageApp, vec![]))),
            1,
        );
        state.apply(
            from_radio(V::MyInfo(pb::MyNodeInfo {
                my_node_num: 2,
                ..Default::default()
            })),
            1,
        );
        assert!(state.nodes.is_empty());
        assert_eq!(state.local.unwrap().num, 2);
    }

    #[test]
    fn channels_keep_no_key() {
        let mut state = ready();
        state.apply(
            from_radio(V::Channel(pb::Channel {
                index: 1,
                settings: Some(pb::ChannelSettings {
                    name: "Galicia".into(),
                    psk: vec![1],
                    ..Default::default()
                }),
                role: pb::channel::Role::Secondary as i32,
            })),
            1,
        );
        assert_eq!(
            state.channels[&1],
            Channel {
                index: 1,
                name: "Galicia".into(),
                role: "SECONDARY".into()
            }
        );
    }
}
