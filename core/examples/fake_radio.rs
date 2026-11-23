// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend Meshtastic radio on TCP, for working on meshctl without one, or
//! without risking one: it sends every part of the configuration and every
//! module these protobufs know, answers admin requests with a passkey, and
//! takes in what it is told to change, so that reading it back shows it.
//! It transmits nothing, anywhere.
//!
//! Its one neighbour answers what it is asked for, after a few seconds:
//! who it is, where, its device telemetry, environment and local stats (it
//! has no power sensors, so `NO_RESPONSE` to that), its neighbours, and
//! traceroutes. As a node does, it keeps from answering its node info more
//! than once a minute, and its position or neighbours once in three; as the
//! firmware does, the radio drops positions and telemetry sent within 10
//! seconds of the last, traceroutes within 30 and texts within 2.
//!
//!     cargo run -p meshctl-core --example fake_radio -- [port] [control port]
//!
//! With a control port, it listens there, on this machine only, for lines
//! that play what another app or the mesh would do to it:
//!
//! - `hear <slot> <text>`: a node of its mesh sends that on a channel.
//! - `past <slot>`: messages on a channel heard over the last year and
//!   more, in their order, as days for a conversation to show.
//! - `fill`: every slot a channel, and the link dropped to have them read.
//! - `dms`: nodes that say who they are and write to the radio directly.
//! - `chat <slot>`: messages on a channel that come every way: directly,
//!   through some hops of those they could take, through MQTT; and, after a
//!   client wrote, a reply to its last text and reactions to it.
//! - `twin <from> <to>`: the channel in one slot is copied into another, the
//!   very same channel twice, as another app may leave it.
//! - `off <slot>`: the channel in a slot is switched off.
//! - `drop`: the link to every client drops, which reads it all anew.
//! - `own`: the radio tells its client its own device telemetry and how it
//!   hears the mesh, as it does now and then.
//! - `place`: the neighbour says where it is, as when asked.
//! - `status <text>`: the neighbour says that of itself, as its status
//!   message; with no text, nothing.
//! - `answers always|sometimes|never`: how the neighbour answers. Always is
//!   within one to six seconds; sometimes, four times in ten not at all and
//!   otherwise within up to 26 seconds.
//! - `limits on|off`: whether the neighbour keeps from answering too often.
//! - `history <hours>`: the neighbour's telemetry and signal over the last so
//!   many hours, as heard then, for charts to show.

use meshctl_proto::descriptors;
use meshctl_proto::frame::{self, FrameParser};
use meshctl_proto::meshtastic::{
    self as pb, AdminMessage, FromRadio, MeshPacket, PortNum, ToRadio, admin_message, config,
    from_radio, mesh_packet, module_config, to_radio,
};
use prost::Message;
use prost_reflect::{DynamicMessage, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

const ME: u32 = 0x1234_abcd;
const PASSKEY: [u8; 8] = [7; 8];
const BROADCAST: u32 = 0xffff_ffff;
/// The node of its mesh heard directly, who `hear` has speak.
const NEIGHBOUR: u32 = 0x0bad_cafe;
/// A repeater further away, which takes no messages.
const REPEATER: u32 = 0x0dec_0de5;

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
    /// How the neighbour answers what it is asked.
    answers: Answers,
    /// Whether the neighbour keeps from answering too often, as nodes do.
    limits: bool,
    /// When the neighbour last answered, by port.
    answered: BTreeMap<i32, Instant>,
    /// When a client last sent on a paced port, which the radio takes no
    /// more often than the firmware does.
    paced: BTreeMap<i32, Instant>,
    /// The nodes marked favourites.
    favorites: std::collections::BTreeSet<u32>,
    /// The nodes a client had it forget: left out of its node list.
    removed: std::collections::BTreeSet<u32>,
    /// The last text a client sent, for the mesh's examples to answer.
    last_text: Option<u32>,
    /// Contacts a client added, as nodes it keeps.
    contacts: BTreeMap<u32, pb::User>,
}

/// How the neighbour answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answers {
    /// Always, within a few seconds.
    Always,
    /// Most of the time, sometimes late.
    Sometimes,
    Never,
}

/// A number from 0 up to, not including, `below`: not for secrets, only so
/// that answers come at their own time.
fn random(below: u64) -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x2545_f491, |d| d.as_nanos() as u64)
            | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x % below.max(1)
}

/// Who the neighbour says it is.
fn neighbour() -> pb::User {
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

/// A packet's id from one of the counters, each with its own top byte: the
/// rest moved on by when this run started, so that a run does not use the
/// ids of the one before, which a client would take for the same packets
/// heard again.
fn fresh_id(counter: &AtomicU32) -> u32 {
    static SALT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let salt = *SALT.get_or_init(|| now_secs().wrapping_mul(97) & 0x00ff_ffff);
    let next = counter.fetch_add(1, Ordering::Relaxed);
    (next & 0xff00_0000) | (next.wrapping_add(salt) & 0x00ff_ffff)
}

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
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
            answers: Answers::Always,
            limits: true,
            answered: BTreeMap::new(),
            paced: BTreeMap::new(),
            favorites: std::collections::BTreeSet::new(),
            removed: std::collections::BTreeSet::new(),
            last_text: None,
            contacts: BTreeMap::new(),
        }
    }

    /// Whether a client sent on a paced port too soon after the last time,
    /// as the firmware counts it: it drops such a packet without a word.
    fn too_soon(&mut self, port: i32) -> bool {
        let interval = match PortNum::try_from(port) {
            Ok(PortNum::TracerouteApp) => Duration::from_secs(30),
            Ok(PortNum::PositionApp | PortNum::TelemetryApp) => Duration::from_secs(10),
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
    fn ask(&mut self, packet: &MeshPacket, data: &pb::Data) -> Option<(Duration, MeshPacket)> {
        use pb::telemetry::Variant as T;
        if self.too_soon(data.portnum) {
            println!("port {} sent too soon: dropped", data.portnum);
            return None;
        }
        // A text that wants an acknowledgement gets one, as the firmware
        // gives it: on a channel, from the radio itself once it hears a
        // neighbour pass it on; to the neighbour, from the neighbour once it
        // has it; to anyone else, after its tries, that nobody answered.
        if data.portnum == PortNum::TextMessageApp as i32 && data.emoji == 0 {
            self.last_text = Some(packet.id);
        }
        if data.portnum == PortNum::TextMessageApp as i32 && packet.want_ack {
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
                    time: now_secs(),
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
                        time: now_secs(),
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
    fn reply(&self, packet: &MeshPacket, port: PortNum, payload: Vec<u8>, snr: f32) -> MeshPacket {
        static NEXT_ID: AtomicU32 = AtomicU32::new(0x6000_0000);
        MeshPacket {
            from: NEIGHBOUR,
            to: ME,
            channel: packet.channel,
            id: fresh_id(&NEXT_ID),
            rx_time: Some(now_secs()),
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
                hops_away: Some(2),
                last_heard: now_secs() - 600,
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

/// What the neighbour sent over the last so many hours, as heard then:
/// device telemetry every quarter of an hour, its battery running down and
/// charging by day, and environment telemetry every half hour, warmer by day.
fn history(hours: u32) -> Vec<MeshPacket> {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0x7000_0000);
    let now = now_secs();
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
                id: fresh_id(&NEXT_ID),
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
        Some("past") => {
            let Some(channel) = slot(words.next()) else {
                return "usage: past <slot>".into();
            };
            // Days ago, and the hour of that day, as it was heard then.
            let said = [
                (400, 9, "Hace más de un año"),
                (400, 18, "Aquel mismo día, por la tarde"),
                (20, 11, "Hace unas semanas"),
                (3, 8, "Hace unos días, por la mañana"),
                (3, 22, "Ese día, de noche"),
                (1, 13, "Ayer a mediodía"),
                (0, 0, "Hoy mismo"),
            ];
            let today = now_secs() - now_secs() % 86_400;
            for (days, hour, text) in said {
                let at = if days == 0 {
                    now_secs()
                } else {
                    today - days * 86_400 + hour * 3_600 + random(1_800) as u32
                };
                let packet = MeshPacket {
                    from: NEIGHBOUR,
                    to: BROADCAST,
                    channel: channel as u32,
                    id: fresh_id(&NEXT_ID),
                    rx_time: Some(at),
                    rx_snr: 6.0,
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
            }
            format!("{} messages from the past on slot {channel}", said.len())
        }
        Some("chat") => {
            let Some(channel) = slot(words.next()) else {
                return "usage: chat <slot>".into();
            };
            // From whom, how far (hops left of those it started with, the
            // last relay), and what: as many ways as a message comes.
            struct Example {
                from: u32,
                hops: (u32, u32),
                relay: u32,
                mqtt: bool,
                text: &'static str,
                reply: bool,
                emoji: bool,
            }
            let example = |from, hops, relay, mqtt, text| Example {
                from,
                hops,
                relay,
                mqtt,
                text,
                reply: false,
                emoji: false,
            };
            let mut examples = vec![
                example(NEIGHBOUR, (3, 3), 0, false, "Hola, te oigo directo"),
                example(0x0dec_0de6, (2, 3), 0xfe, false, "Por aquí, a un salto"),
                example(0x0dec_0de7, (0, 3), 0xca, false, "Llego justo: tres saltos de tres"),
                example(0x0dec_0de8, (5, 7), 0xfe, false, "Dos saltos de siete"),
                example(0x0dec_0de9, (3, 3), 0, true, "Desde MQTT"),
            ];
            let last = radio.lock().unwrap().last_text;
            if last.is_some() {
                examples.push(Example {
                    reply: true,
                    ..example(NEIGHBOUR, (3, 3), 0, false, "Respondo a lo último que escribiste")
                });
                for (from, emoji) in [(NEIGHBOUR, "👍"), (0x0dec_0de6, "❤️")] {
                    examples.push(Example {
                        reply: true,
                        emoji: true,
                        ..example(from, (2, 3), 0xfe, false, emoji)
                    });
                }
            }
            let count = examples.len();
            let events = events.clone();
            tokio::spawn(async move {
                for example in examples {
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                    let packet = MeshPacket {
                        from: example.from,
                        to: BROADCAST,
                        channel: channel as u32,
                        id: fresh_id(&NEXT_ID),
                        rx_time: Some(now_secs()),
                        rx_snr: 2.0 + random(80) as f32 / 10.0,
                        rx_rssi: Some(-60 - random(40) as i32),
                        hop_limit: example.hops.0,
                        hop_start: example.hops.1,
                        relay_node: example.relay,
                        via_mqtt: example.mqtt,
                        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                            portnum: PortNum::TextMessageApp as i32,
                            payload: example.text.as_bytes().to_vec(),
                            reply_id: if example.reply { last.unwrap_or_default() } else { 0 },
                            emoji: u32::from(example.emoji),
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    let _ = events.send(Event::Heard(packet));
                }
            });
            format!("{count} messages on slot {channel}, one every 1.5 s")
        }
        Some("fill") => {
            // Every slot a channel, as a radio that has them all; one with
            // a long name, to be cut.
            let names = [
                "Galicia",
                "Emergencias",
                "Montaña",
                "Vecinos",
                "Radioaficionados",
                "Pruebas con un nombre larguísimo",
            ];
            let mut radio = radio.lock().unwrap();
            for (index, name) in (2..8).zip(names) {
                radio.channels.insert(
                    index,
                    pb::Channel {
                        index,
                        role: pb::channel::Role::Secondary as i32,
                        settings: Some(pb::ChannelSettings {
                            name: name.into(),
                            psk: vec![index as u8; 32],
                            ..Default::default()
                        }),
                    },
                );
            }
            drop(radio);
            let _ = events.send(Event::Drop);
            "slots 2 to 7 are channels now; dropped to have them read".into()
        }
        Some("dms") => {
            // Who they are, and what each writes to the radio.
            let nodes: [(u32, &str, &str, &[&str]); 9] = [
                (0x0dec_1001, "Ana Coruña", "ANA", &["Hola, ¿me lees?", "Te oigo bien desde casa"]),
                (0x0dec_1002, "Bici de Bea", "BEA", &["Saliendo hacia el monte"]),
                (0x0dec_1003, "Repetidor Monte Xalo", "XALO", &["Batería al 80 %", "Todo en orden"]),
                (0x0dec_1004, "Coche de Luis", "LUIS", &["En la autopista, llego en media hora"]),
                (0x0dec_1005, "Base Ferrol", "FERR", &["¿Alguien en la frecuencia?"]),
                (0x0dec_1006, "María", "MAR", &["Gracias por el aviso", "Nos vemos mañana", "👍"]),
                (0x0dec_1007, "Estación meteorológica", "METE", &["Mañana lluvia"]),
                (0x0dec_1008, "Un nodo con un nombre larguísimo para ver cómo se corta", "LARG", &["Probando nombres largos"]),
                (0x0dec_1009, "Pedro", "PED", &["¿Quedamos para probar antenas?"]),
            ];
            let count = nodes.iter().map(|(_, _, _, said)| said.len()).sum::<usize>();
            let events = events.clone();
            tokio::spawn(async move {
                for (num, long, short, said) in nodes {
                    let user = pb::User {
                        id: format!("!{num:08x}"),
                        long_name: long.into(),
                        short_name: short.into(),
                        hw_model: pb::HardwareModel::HeltecV3 as i32,
                        public_key: num.to_be_bytes().repeat(8),
                        ..Default::default()
                    };
                    let heard = |to, port: PortNum, payload: Vec<u8>| MeshPacket {
                        from: num,
                        to,
                        channel: 0,
                        id: fresh_id(&NEXT_ID),
                        rx_time: Some(now_secs()),
                        rx_snr: 2.0 + random(80) as f32 / 10.0,
                        rx_rssi: Some(-60 - random(40) as i32),
                        hop_limit: 3 - (num % 3),
                        hop_start: 3,
                        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                            portnum: port as i32,
                            payload,
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    let _ = events.send(Event::Heard(heard(
                        BROADCAST,
                        PortNum::NodeinfoApp,
                        user.encode_to_vec(),
                    )));
                    for text in said {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let _ = events.send(Event::Heard(heard(
                            ME,
                            PortNum::TextMessageApp,
                            text.as_bytes().to_vec(),
                        )));
                    }
                }
            });
            format!("{count} direct messages from {} nodes", nodes.len())
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
        Some("place") => {
            // Where it answers it is when asked, of its own accord.
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now_secs()),
                rx_snr: 7.5,
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::PositionApp as i32,
                    payload: pb::Position {
                        latitude_i: Some(404_168_000 + random(2_000) as i32),
                        longitude_i: Some(-37_038_000 + random(2_000) as i32),
                        altitude: Some(650 + random(20) as i32),
                        time: now_secs(),
                        location_source: pb::position::LocSource::LocInternal as i32,
                        precision_bits: 32,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            "the neighbour says where it is".into()
        }
        Some("status") => {
            let text = words.collect::<Vec<_>>().join(" ");
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now_secs()),
                rx_snr: 7.5,
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::NodeStatusApp as i32,
                    payload: pb::StatusMessage {
                        status: text.clone(),
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("the neighbour says «{text}»")
        }
        Some("own") => {
            // As the firmware tells its client now and then: its device
            // telemetry, sent to the mesh too, and how it hears it.
            let now = now_secs();
            let variants = [
                pb::telemetry::Variant::DeviceMetrics(pb::DeviceMetrics {
                    battery_level: Some(20 + random(80) as u32),
                    voltage: Some(3.5 + random(70) as f32 / 100.0),
                    channel_utilization: Some(random(200) as f32 / 10.0),
                    air_util_tx: Some(random(50) as f32 / 10.0),
                    uptime_seconds: Some(3_600 + random(86_400) as u32),
                }),
                pb::telemetry::Variant::LocalStats(pb::LocalStats {
                    uptime_seconds: 3_600 + random(86_400) as u32,
                    channel_utilization: random(200) as f32 / 10.0,
                    air_util_tx: random(50) as f32 / 10.0,
                    num_packets_tx: 100 + random(900) as u32,
                    num_packets_rx: 1_000 + random(9_000) as u32,
                    num_packets_rx_bad: random(50) as u32,
                    num_online_nodes: 5 + random(30) as u32,
                    num_total_nodes: 40 + random(60) as u32,
                    noise_floor: -110 + random(15) as i32,
                    ..Default::default()
                }),
            ];
            for variant in variants {
                let to = if matches!(variant, pb::telemetry::Variant::LocalStats(_)) {
                    ME
                } else {
                    BROADCAST
                };
                let packet = MeshPacket {
                    from: ME,
                    to,
                    id: fresh_id(&NEXT_ID),
                    rx_time: Some(now),
                    payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                        portnum: PortNum::TelemetryApp as i32,
                        payload: pb::Telemetry {
                            time: now,
                            variant: Some(variant),
                        }
                        .encode_to_vec(),
                        ..Default::default()
                    })),
                    ..Default::default()
                };
                let _ = events.send(Event::Heard(packet));
            }
            "its own telemetry and stats, told".into()
        }
        Some("history") => {
            let Some(hours) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return "usage: history <hours>".into();
            };
            let packets = history(hours);
            let count = packets.len();
            // A few at a time, as clients keep only so many events behind.
            let events = events.clone();
            tokio::spawn(async move {
                for packet in packets {
                    let _ = events.send(Event::Heard(packet));
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            format!("{count} packets of the neighbour's last {hours} hours, on their way")
        }
        Some("answers") => {
            let answers = match words.next() {
                Some("always") => Answers::Always,
                Some("sometimes") => Answers::Sometimes,
                Some("never") => Answers::Never,
                _ => return "usage: answers always|sometimes|never".into(),
            };
            radio.lock().unwrap().answers = answers;
            format!("the neighbour answers: {answers:?}")
        }
        Some("limits") => {
            let limits = match words.next() {
                Some("on") => true,
                Some("off") => false,
                _ => return "usage: limits on|off".into(),
            };
            let mut radio = radio.lock().unwrap();
            radio.limits = limits;
            radio.answered.clear();
            format!("the neighbour keeps from answering too often: {limits}")
        }
        _ => "hear <slot> <text> | twin <from> <to> | off <slot> | drop | answers always|sometimes|never | limits on|off | history <hours>".into(),
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
        let events = events.clone();
        tokio::spawn(async move {
            if let Err(error) = serve(stream, radio, events).await {
                println!("client {address} gone: {error}");
            }
        });
    }
}
