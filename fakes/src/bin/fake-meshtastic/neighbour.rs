// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's one neighbour, heard directly: who it is, how it answers
//! what it is asked, and what it said over the last hours.

use meshctl_fakes::{now, random};
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, MeshPacket, PortNum, admin_message, config, mesh_packet,
};
use prost::Message;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

use crate::radio::{
    BROADCAST, ME, NEIGHBOUR, NEIGHBOUR_PASSKEY, REPEATER, Radio, fresh_id, part_name,
};

/// The neighbour's settings, which its admin reads and changes.
pub struct NeighbourSettings {
    pub owner: pb::User,
    pub configs: BTreeMap<String, pb::Config>,
    pub modules: BTreeMap<String, pb::ModuleConfig>,
    pub channels: BTreeMap<i32, pb::Channel>,
    /// The nodes it knows, by number: whether a favourite, whether ignored.
    pub nodes: BTreeMap<u32, (bool, bool)>,
}

/// How the neighbour answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answers {
    /// Always, within a few seconds.
    Always,
    /// Most of the time, sometimes late.
    Sometimes,
    Never,
}

/// Who the neighbour says it is.
pub fn neighbour() -> pb::User {
    pb::User {
        id: format!("!{NEIGHBOUR:08x}"),
        long_name: "Fake neighbour".into(),
        short_name: "NBR".into(),
        hw_model: pb::HardwareModel::WismeshTag as i32,
        role: config::device_config::Role::Router as i32,
        // Made up, as all of it: no private key goes with it.
        public_key: (1..=32).collect(),
        ..Default::default()
    }
}

impl Radio {
    /// What the neighbour answers an admin request to it, and after how
    /// long, if it is sealed to it and it takes the radio's key: part of
    /// its settings, asked for; a change to them, with the passkey it gave
    /// out, acknowledged and kept. The routing error the firmware gives, if
    /// not.
    pub fn neighbour_admin(
        &mut self,
        packet: &MeshPacket,
        admin: AdminMessage,
    ) -> Option<(Duration, MeshPacket)> {
        use admin_message::PayloadVariant as A;
        let delay = Duration::from_millis(1_500 + random(3_000));
        let routed = |radio: &Self, error: pb::routing::Error| {
            let routing = pb::Routing {
                variant: Some(pb::routing::Variant::ErrorReason(error as i32)),
                ..Default::default()
            };
            Some((
                delay,
                radio.reply(packet, PortNum::RoutingApp, routing.encode_to_vec(), 6.0),
            ))
        };
        if !packet.pki_encrypted {
            println!("neighbour refuses admin: not sealed");
            return routed(self, pb::routing::Error::NotAuthorized);
        }
        if !self.admin_key {
            println!("neighbour refuses admin: not its admin key");
            return routed(self, pb::routing::Error::AdminPublicKeyUnauthorized);
        }
        // Its own settings, the radio's as they were when first asked.
        let settings = self.neighbour.get_or_insert_with(|| NeighbourSettings {
            owner: neighbour(),
            configs: self.configs.clone(),
            modules: self.modules.clone(),
            channels: self.channels.clone(),
            // The radio, and the repeater beyond it.
            nodes: BTreeMap::from([(ME, (false, false)), (REPEATER, (false, false))]),
        });
        // A part by its request's number, one less than the protobufs'.
        let part = |message: &str, number: i32| {
            meshctl_proto::descriptors()
                .get_message_by_name(message)?
                .get_field((number + 1) as u32)
                .map(|f| f.name().to_owned())
        };
        let asked = matches!(
            admin.payload_variant,
            Some(
                A::GetOwnerRequest(_)
                    | A::GetDeviceMetadataRequest(_)
                    | A::GetConfigRequest(_)
                    | A::GetModuleConfigRequest(_)
                    | A::GetChannelRequest(_)
            )
        );
        if !asked && admin.session_passkey != NEIGHBOUR_PASSKEY {
            println!("neighbour refuses admin: wrong passkey");
            return routed(self, pb::routing::Error::AdminBadSessionKey);
        }
        let answer = match admin.payload_variant? {
            A::GetOwnerRequest(_) => A::GetOwnerResponse(settings.owner.clone()),
            A::GetDeviceMetadataRequest(_) => A::GetDeviceMetadataResponse(pb::DeviceMetadata {
                firmware_version: "0.0.0.fake".into(),
                hw_model: pb::HardwareModel::WismeshTag as i32,
                has_wifi: true,
                has_bluetooth: true,
                ..Default::default()
            }),
            A::GetConfigRequest(number) => A::GetConfigResponse(
                settings
                    .configs
                    .get(&part("meshtastic.Config", number)?)?
                    .clone(),
            ),
            A::GetModuleConfigRequest(number) => {
                let name = part("meshtastic.ModuleConfig", number)?;
                // A firmware from before status messages knows not what it
                // is asked for: nothing comes back.
                if self.old_firmware && name == "statusmessage" {
                    println!("neighbour, an old firmware, says nothing of a status message");
                    return None;
                }
                A::GetModuleConfigResponse(settings.modules.get(&name)?.clone())
            }
            A::GetChannelRequest(asked) => {
                let index = asked as i32 - 1;
                A::GetChannelResponse(settings.channels.get(&index).cloned().unwrap_or(
                    pb::Channel {
                        index,
                        ..Default::default()
                    },
                ))
            }
            // Changes, kept, and acknowledged by the neighbour itself.
            change => {
                match change {
                    A::SetOwner(owner) => {
                        println!(
                            "neighbour owner: {} / {}",
                            owner.long_name, owner.short_name
                        );
                        settings.owner = owner;
                    }
                    A::SetConfig(config) => {
                        let name = part_name("meshtastic.Config", &config)?;
                        println!("neighbour config.{name}: {config:?}");
                        settings.configs.insert(name, config);
                    }
                    A::SetModuleConfig(module) => {
                        let name = part_name("meshtastic.ModuleConfig", &module)?;
                        println!("neighbour module.{name}: {module:?}");
                        settings.modules.insert(name, module);
                    }
                    A::SetChannel(channel) => {
                        println!(
                            "neighbour channel {}: {:?}",
                            channel.index,
                            channel.settings.as_ref().map(|s| &s.name)
                        );
                        settings.channels.insert(channel.index, channel);
                    }
                    A::BeginEditSettings(_) => println!("neighbour begins an edit"),
                    A::RebootSeconds(seconds) => println!("neighbour reboots in {seconds} s"),
                    A::ShutdownSeconds(seconds) => {
                        println!("neighbour shuts down in {seconds} s");
                    }
                    A::NodedbReset(_) => println!("neighbour forgets its nodes"),
                    A::SetFixedPosition(position) => println!(
                        "neighbour fixed at {:?}, {:?}, {:?} m",
                        position.latitude_i, position.longitude_i, position.altitude
                    ),
                    A::RemoveFixedPosition(_) => println!("neighbour's fixed position removed"),
                    A::FactoryResetConfig(_) => {
                        println!("neighbour goes back to its factory settings");
                        self.neighbour = None;
                        return routed(self, pb::routing::Error::None);
                    }
                    A::CommitEditSettings(_) => {
                        println!("neighbour commits the edit (a real node might reboot now)");
                    }
                    // Its own list of nodes, as the firmware keeps it: a
                    // contact a favourite but on a Client Base; a favourite,
                    // or any but a block, only of one it knows.
                    A::AddContact(contact) => {
                        let client_base = matches!(
                            settings.configs.get("device").and_then(|c| c.payload_variant.as_ref()),
                            Some(config::PayloadVariant::Device(d))
                                if d.role == config::device_config::Role::ClientBase as i32
                        );
                        let entry = settings.nodes.entry(contact.node_num).or_default();
                        if !client_base {
                            entry.0 = true;
                        }
                        println!(
                            "neighbour keeps !{:08x} as a contact{}",
                            contact.node_num,
                            if client_base {
                                ", a Client Base: no favourite"
                            } else {
                                ", a favourite"
                            }
                        );
                    }
                    A::SetFavoriteNode(num) => match settings.nodes.get_mut(&num) {
                        Some(entry) => {
                            entry.0 = true;
                            println!("neighbour favours !{num:08x}");
                        }
                        None => println!("neighbour knows no !{num:08x}: no favourite"),
                    },
                    A::RemoveFavoriteNode(num) => {
                        if let Some(entry) = settings.nodes.get_mut(&num) {
                            entry.0 = false;
                        }
                        println!("neighbour no longer favours !{num:08x}");
                    }
                    A::SetIgnoredNode(num) => {
                        settings.nodes.entry(num).or_default().1 = true;
                        println!("neighbour ignores !{num:08x}");
                    }
                    A::RemoveIgnoredNode(num) => {
                        if let Some(entry) = settings.nodes.get_mut(&num) {
                            entry.1 = false;
                        }
                        println!("neighbour no longer ignores !{num:08x}");
                    }
                    A::RemoveByNodenum(num) => {
                        settings.nodes.remove(&num);
                        println!("neighbour forgets !{num:08x}");
                    }
                    other => println!("neighbour admin, ignored: {other:?}"),
                }
                return routed(self, pb::routing::Error::None);
            }
        };
        println!("neighbour answers admin in {} ms", delay.as_millis());
        let mut reply = self.reply(
            packet,
            PortNum::AdminApp,
            AdminMessage {
                session_passkey: NEIGHBOUR_PASSKEY.to_vec(),
                payload_variant: Some(answer),
            }
            .encode_to_vec(),
            6.0,
        );
        reply.pki_encrypted = true;
        Some((delay, reply))
    }

    /// Whether a client sent on a paced port too soon after the last time,
    /// as the firmware counts it: it drops such a packet without a word.
    pub fn too_soon(&mut self, port: i32) -> bool {
        let interval = match PortNum::try_from(port) {
            Ok(PortNum::TracerouteApp) => Duration::from_secs(30),
            Ok(PortNum::PositionApp | PortNum::TelemetryApp | PortNum::WaypointApp) => {
                Duration::from_secs(10)
            }
            Ok(PortNum::TextMessageApp) => Duration::from_secs(2),
            _ => return false,
        };
        if self
            .paced
            .get(&port)
            .is_some_and(|at| at.elapsed() < interval)
        {
            return true;
        }
        self.paced.insert(port, Instant::now());
        false
    }

    /// What the neighbour answers a packet sent to it that wants one, and
    /// after how long; none when it keeps from answering, or the radio
    /// dropped it.
    pub fn ask(&mut self, packet: &MeshPacket, data: &pb::Data) -> Option<(Duration, MeshPacket)> {
        use pb::telemetry::Variant as T;
        if self.too_soon(data.portnum) {
            // A waypoint is refused out loud, as the firmware does: a
            // routing error for it, at once.
            if data.portnum == PortNum::WaypointApp as i32 {
                println!("waypoint sent too soon: refused");
                let routing = pb::Routing {
                    variant: Some(pb::routing::Variant::ErrorReason(
                        pb::routing::Error::RateLimitExceeded as i32,
                    )),
                    ..Default::default()
                };
                let mut refused =
                    self.reply(packet, PortNum::RoutingApp, routing.encode_to_vec(), 0.0);
                refused.from = ME;
                return Some((Duration::from_millis(50), refused));
            }
            println!("port {} sent too soon: dropped", data.portnum);
            return None;
        }
        // A text that wants an acknowledgement gets one, as the firmware
        // gives it: on a channel, from the radio itself once it hears a
        // neighbour pass it on; to the neighbour, from the neighbour once it
        // has it; to anyone else, after its tries, that nobody answered. A
        // position on a channel, the same; to the neighbour, its own answers
        // it.
        if data.portnum == PortNum::TextMessageApp as i32 && data.emoji == 0 {
            self.last_text = Some(packet.id);
        }
        let acked = data.portnum == PortNum::TextMessageApp as i32
            || ((data.portnum == PortNum::PositionApp as i32
                || data.portnum == PortNum::WaypointApp as i32)
                && packet.to == u32::MAX);
        if data.portnum == PortNum::WaypointApp as i32
            && let Ok(waypoint) = pb::Waypoint::decode(data.payload.as_slice())
        {
            println!(
                "waypoint {:08x} \"{}\" sent, expire {}, hop limit {}",
                waypoint.id, waypoint.name, waypoint.expire, packet.hop_limit
            );
        }
        if acked && packet.want_ack {
            let (error, delay) = if packet.to == NEIGHBOUR || packet.to == u32::MAX {
                (
                    pb::routing::Error::None,
                    Duration::from_millis(800 + random(1_500)),
                )
            } else {
                (pb::routing::Error::MaxRetransmit, Duration::from_secs(8))
            };
            let routing = pb::Routing {
                variant: Some(pb::routing::Variant::ErrorReason(error as i32)),
                ..Default::default()
            };
            let mut ack = self.reply(packet, PortNum::RoutingApp, routing.encode_to_vec(), 6.0);
            if packet.to != NEIGHBOUR {
                ack.from = ME;
            }
            println!(
                "text to !{:08x}: {error:?} in {} ms",
                packet.to,
                delay.as_millis()
            );
            return Some((delay, ack));
        }
        if packet.to != NEIGHBOUR || !data.want_response {
            return None;
        }
        let port = PortNum::try_from(data.portnum).ok()?;
        // As nodes keep from answering the same too often, whoever asks.
        let quiet = match port {
            PortNum::NodeinfoApp => Duration::from_secs(60),
            PortNum::PositionApp | PortNum::NeighborinfoApp => Duration::from_secs(180),
            _ => Duration::ZERO,
        };
        if self.limits
            && self
                .answered
                .get(&data.portnum)
                .is_some_and(|at| at.elapsed() < quiet)
        {
            println!("neighbour answered port {} lately: quiet", data.portnum);
            return None;
        }
        let delay = match self.answers {
            Answers::Never => {
                println!("neighbour does not answer");
                return None;
            }
            Answers::Always => Duration::from_millis(1_000 + random(5_000)),
            Answers::Sometimes => {
                if random(10) < 4 {
                    println!("neighbour does not answer, this time");
                    return None;
                }
                Duration::from_millis(1_000 + random(25_000))
            }
        };
        let snr = 4.0 + random(80) as f32 / 10.0;
        let (port, payload) = match port {
            PortNum::NodeinfoApp => (port, neighbour().encode_to_vec()),
            PortNum::PositionApp => (
                port,
                pb::Position {
                    latitude_i: Some(404_168_000 + random(2_000) as i32),
                    longitude_i: Some(-37_038_000 + random(2_000) as i32),
                    altitude: Some(650 + random(20) as i32),
                    time: now(),
                    location_source: pb::position::LocSource::LocInternal as i32,
                    precision_bits: 32,
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            PortNum::TelemetryApp => {
                let asked = pb::Telemetry::decode(data.payload.as_slice()).ok()?;
                let variant = match asked.variant? {
                    T::DeviceMetrics(_) => T::DeviceMetrics(pb::DeviceMetrics {
                        battery_level: Some(60 + random(40) as u32),
                        voltage: Some(3.7 + random(50) as f32 / 100.0),
                        channel_utilization: Some(random(150) as f32 / 10.0),
                        air_util_tx: Some(random(30) as f32 / 10.0),
                        uptime_seconds: Some(3_600 + random(86_400) as u32),
                    }),
                    T::EnvironmentMetrics(_) => T::EnvironmentMetrics(pb::EnvironmentMetrics {
                        temperature: Some(15.0 + random(100) as f32 / 10.0),
                        relative_humidity: Some(40.0 + random(400) as f32 / 10.0),
                        barometric_pressure: Some(1_000.0 + random(300) as f32 / 10.0),
                        ..Default::default()
                    }),
                    T::LocalStats(_) => T::LocalStats(pb::LocalStats {
                        uptime_seconds: 3_600 + random(86_400) as u32,
                        channel_utilization: random(150) as f32 / 10.0,
                        air_util_tx: random(30) as f32 / 10.0,
                        num_packets_tx: 100 + random(900) as u32,
                        num_packets_rx: 1_000 + random(9_000) as u32,
                        num_packets_rx_bad: random(50) as u32,
                        num_online_nodes: 5 + random(30) as u32,
                        num_total_nodes: 40 + random(60) as u32,
                        noise_floor: -110 + random(15) as i32,
                        ..Default::default()
                    }),
                    // No power sensors: nothing to answer with.
                    _ => {
                        let routing = pb::Routing {
                            variant: Some(pb::routing::Variant::ErrorReason(
                                pb::routing::Error::NoResponse as i32,
                            )),
                            ..Default::default()
                        };
                        return Some((
                            delay,
                            self.reply(packet, PortNum::RoutingApp, routing.encode_to_vec(), snr),
                        ));
                    }
                };
                (
                    port,
                    pb::Telemetry {
                        time: now(),
                        variant: Some(variant),
                    }
                    .encode_to_vec(),
                )
            }
            PortNum::NeighborinfoApp => (
                port,
                pb::NeighborInfo {
                    node_id: NEIGHBOUR,
                    last_sent_by_id: NEIGHBOUR,
                    node_broadcast_interval_secs: 900,
                    // Us, and as many others as a busy router hears (more
                    // than a node page shows before "see all"), unknown to
                    // the radio but one.
                    neighbors: std::iter::once(pb::Neighbor {
                        node_id: ME,
                        snr,
                        ..Default::default()
                    })
                    .chain((0..8).map(|i| pb::Neighbor {
                        node_id: 0x0dec_0de5 + i,
                        snr: -12.0 + random(180) as f32 / 10.0,
                        ..Default::default()
                    }))
                    .collect(),
                }
                .encode_to_vec(),
            ),
            PortNum::TracerouteApp => (
                port,
                pb::RouteDiscovery {
                    snr_towards: vec![(snr * 4.0) as i32],
                    snr_back: vec![((snr - 1.0) * 4.0) as i32],
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            _ => return None,
        };
        self.answered.insert(data.portnum, Instant::now());
        Some((delay, self.reply(packet, port, payload, snr)))
    }

    /// The neighbour's answer to a packet, heard straight from it.
    pub fn reply(
        &self,
        packet: &MeshPacket,
        port: PortNum,
        payload: Vec<u8>,
        snr: f32,
    ) -> MeshPacket {
        static NEXT_ID: AtomicU32 = AtomicU32::new(0x6000_0000);
        MeshPacket {
            from: NEIGHBOUR,
            to: ME,
            channel: packet.channel,
            id: fresh_id(&NEXT_ID),
            rx_time: Some(now()),
            rx_snr: snr,
            rx_rssi: Some(-60 - random(40) as i32),
            hop_limit: 3,
            hop_start: 3,
            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                portnum: port as i32,
                payload,
                request_id: packet.id,
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}

/// What the neighbour sent over the last so many hours, as heard then:
/// device telemetry every quarter of an hour, its battery running down and
/// charging by day, and environment telemetry every half hour, warmer by day.
pub fn history(hours: u32) -> Vec<MeshPacket> {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0x7000_0000);
    let now = now();
    let step = 15 * 60;
    let steps = hours * 3600 / step;
    let mut packets = Vec::new();
    for i in (1..=steps).rev() {
        let at = now - i * step;
        // Hours into the day, and a wave over it: lowest at four, highest
        // at four in the afternoon.
        let hour = f64::from(at % 86_400) / 3600.0;
        let day = (std::f64::consts::TAU * (hour - 10.0) / 24.0).sin();
        let battery = (70.0 + 25.0 * day + random(4) as f64).clamp(5.0, 100.0);
        let snr = 6.0 + 3.0 * day + random(30) as f64 / 10.0 - 1.5;
        let telemetry = |variant| pb::Telemetry {
            time: at,
            variant: Some(variant),
        };
        let mut variants = vec![pb::telemetry::Variant::DeviceMetrics(pb::DeviceMetrics {
            battery_level: Some(battery as u32),
            voltage: Some((3.3 + battery / 100.0 * 0.9) as f32),
            channel_utilization: Some((4.0 + 3.0 * (day + 1.0) + random(20) as f64 / 10.0) as f32),
            air_util_tx: Some((0.5 + random(15) as f64 / 10.0) as f32),
            uptime_seconds: Some(at.saturating_sub(now - hours * 3600)),
        })];
        if i % 2 == 0 {
            variants.push(pb::telemetry::Variant::EnvironmentMetrics(
                pb::EnvironmentMetrics {
                    temperature: Some((17.0 + 6.0 * day + random(10) as f64 / 10.0) as f32),
                    relative_humidity: Some((60.0 - 15.0 * day + random(50) as f64 / 10.0) as f32),
                    barometric_pressure: Some(
                        (1015.0 + 4.0 * (f64::from(at) / 43_200.0).sin()) as f32,
                    ),
                    ..Default::default()
                },
            ));
        }
        for variant in variants {
            // Now and then it is heard through one or two others.
            let hops = if random(4) == 0 {
                1 + random(2) as u32
            } else {
                0
            };
            packets.push(MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(at),
                rx_snr: snr as f32,
                rx_rssi: Some(-75 - random(20) as i32),
                hop_limit: 3 - hops,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::TelemetryApp as i32,
                    payload: telemetry(variant).encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            });
        }
    }
    packets
}
