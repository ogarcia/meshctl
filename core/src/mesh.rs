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
use std::collections::{BTreeMap, BTreeSet};

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
    /// Its public key, 32 bytes, as it sends it; empty when not known. What
    /// it is sent to itself goes encrypted to it, whatever the channel.
    pub public_key: Vec<u8>,
    /// Whether its owner is a licensed radio amateur: then it sends nothing
    /// encrypted, to itself either.
    pub licensed: bool,
    /// Whether it tells that nobody reads messages sent to it, as a
    /// repeater or a sensor of the mesh's own.
    pub unmessagable: bool,
}

/// Where a node says it is, in degrees and metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<i32>,
    /// How far, in metres, it may be from where the node is, when the node
    /// shares it rounded; none when it is exact or does not say.
    pub radius: Option<u32>,
    /// How the node came by it, when it says.
    pub source: Option<PositionSource>,
}

/// How a node came by its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSource {
    /// Set by hand, as a fixed position.
    Manual,
    /// From its own GPS.
    Internal,
    /// From a GPS outside it: a phone, say.
    External,
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
    /// How busy it hears the channel, as a percentage of the time.
    pub channel_utilization: Option<f32>,
    /// Whether the radio has it in its own list, or meshctl alone keeps it,
    /// as heard before: none until the radio has been read in full since
    /// meshctl started, as only then is it known.
    pub on_radio: Option<bool>,
    /// What it says of itself, as it last sent it: none when it never did,
    /// or said nothing.
    pub status: Option<String>,
    /// When it said it, in seconds since the epoch; none for the radio's
    /// own, which its settings say.
    pub status_at: Option<u32>,
}

impl Node {
    pub(crate) fn new(num: u32) -> Self {
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
            channel_utilization: None,
            on_radio: None,
            status: None,
            status_at: None,
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
    /// The radio forgot a node, as asked: out of its node list.
    NodeRemoved(u32),
    /// A packet the radio heard, whatever it carries: what a log of the mesh
    /// keeps. What it means, a message or an answer, comes as its own event
    /// besides.
    Packet(Heard),
    /// A text message heard. Messages are not kept here: whoever wants them,
    /// keeps them.
    Message(TextMessage),
    /// The answer to a packet sent with want_ack: delivered, heard relayed,
    /// or given up on.
    Ack(Ack),
    /// The answer to a traceroute sent.
    Traceroute(TracerouteAnswer),
    /// The radio's answer to an admin request: the session passkey that
    /// changing its settings takes. Taken by whoever asked; never broadcast.
    AdminAnswer {
        request_id: u32,
        passkey: Vec<u8>,
    },
    /// What a node answered a request with that comes as no packet, as
    /// MeshCore nodes answer.
    Reply(Reply),
    /// A copy of a message heard, and the way it came.
    Route(Route),
    /// A packet a MeshCore radio heard, raw, as it logs every copy: for its
    /// live view.
    RawHeard(RawHeard),
    /// What a MeshCore node's console answered the radio, logged in to it as
    /// its admin: kept out of the chat, as it is no message.
    Console {
        from: u32,
        text: String,
        at: u32,
    },
    /// A request to a node given up on, as long as the radio expects its
    /// answer to take gone by with none: by its id, and the node asked.
    Unanswered {
        request_id: u32,
        from: u32,
    },
}

/// A copy of a packet a MeshCore radio heard, raw: what names it whatever
/// way it came, how it came, and what could be read of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RawHeard {
    /// What names the packet in all its copies: its type and payload,
    /// hashed.
    pub key: u64,
    pub payload_type: u8,
    /// 0 flooded with region codes, 1 flooded, 2 direct, 3 direct with
    /// region codes.
    pub route: u8,
    /// The region it went in, for those sent with one: its code, and what
    /// it was worked out from.
    pub region: Option<RegionCode>,
    /// The hashes of the repeaters it went through, as heard, each
    /// `hash_size` bytes: for a flood from its sender's side; for a trace,
    /// the SNR each hop heard it at instead.
    pub path: Vec<u8>,
    pub hash_size: u8,
    pub payload_len: usize,
    pub snr: f32,
    pub rssi: i32,
    /// When it was heard, in seconds since the epoch.
    pub at: u32,
    pub info: RawInfo,
}

/// What could be read of a packet heard raw, by its kind.
#[derive(Debug, Clone, PartialEq)]
pub enum RawInfo {
    /// A node's advert: who it is, by its number, and what it said.
    Advert {
        node: u32,
        kind: u8,
        name: Option<String>,
        position: Option<(f64, f64)>,
        timestamp: u32,
    },
    /// A channel's text, opened with its secret: on which slot, who wrote
    /// it as it says, and what.
    ChannelText {
        channel: u32,
        sender: Option<String>,
        text: String,
        timestamp: u32,
    },
    /// A channel's text the radio has no secret for: its channel's hash.
    Sealed { channel_hash: u8 },
    /// A message, request, answer or path between two nodes, sealed: by the
    /// hashes of whom it is to and from.
    Between { to: u8, from: u8 },
    /// A request with no login: to whom, by its hash, and from whom, by the
    /// number its key makes.
    Anonymous { to: u8, from: u32 },
    /// An acknowledgement: its checksum.
    Ack { code: u32 },
    /// A trace: its tag, and the path it goes along, each hop by a hash of
    /// `hash_size` bytes; the SNR each heard it at is its packet's path.
    Trace {
        tag: u32,
        hashes: Vec<u8>,
        hash_size: u8,
    },
    /// Anything else: only its kind and size.
    Other,
}

/// A copy of a message the radio heard, and the way it came: as MeshCore
/// radios log every one.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// The message it is a copy of.
    pub packet_id: u32,
    /// The hashes of the repeaters it went through, from its sender's side;
    /// none for one heard straight, or sent along a path.
    pub hops: Vec<Vec<u8>>,
    /// Whether it was flooded, its path built up as it went.
    pub flooded: bool,
    pub snr: f32,
    pub rssi: i32,
    /// When it was heard, in seconds since the epoch.
    pub at: u32,
    /// The region code it went with, and what it is the code of.
    pub region: Option<RegionCode>,
}

/// A packet's region code, with what it was worked out from: which region it
/// is, whoever knows the region's key can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionCode {
    pub code: u16,
    pub payload_type: u8,
    pub payload: Vec<u8>,
}

/// The region a MeshCore message floods in, for it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Scoped to a region, by its key.
    Region([u8; 16]),
    /// Scoped to none, whatever the radio's default.
    Unscoped,
}

/// What a node answered a request with, by the id the request went by.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub request_id: u32,
    /// The node asked, which is who answered.
    pub from: u32,
    pub answer: Answer,
}

/// What a node answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The regions a repeater repeats floods scoped to, by their names, and
    /// whether it repeats those scoped to none; with its clock.
    Regions {
        names: Vec<String>,
        unscoped: bool,
        clock: Option<u32>,
    },
    /// How a MeshCore repeater or room is doing.
    Status(crate::meshcore::answers::NodeStatus),
    /// What a MeshCore node's sensors read, its own among them.
    Telemetry(Vec<crate::meshcore::answers::Reading>),
    /// The nodes a MeshCore node hears straight, by their numbers, as far
    /// as the start of their keys tells: how many in all, and those told.
    Neighbours { total: u16, heard: Vec<Neighbour> },
    /// Who runs a MeshCore node, as it says.
    Owner(crate::meshcore::answers::Owner),
    /// Who may log in to a MeshCore node, as its admin is told: each by its
    /// number, as far as the start of its key tells, and the access list's
    /// bits it has.
    AccessList(Vec<(u32, u8)>),
    /// A MeshCore node heard straight, pinged: how long there and back, in
    /// milliseconds, the SNR it heard the radio at, and the SNR the radio
    /// heard it back at.
    Ping {
        round_trip_ms: u32,
        snr_there: f32,
        snr_back: f32,
    },
}

/// A node another hears straight: by its number, how long ago, and at what
/// SNR.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    pub node: u32,
    pub seconds_ago: u32,
    pub snr: f32,
}

/// What the firmware puts in a route for a hop it could not tell.
pub const UNKNOWN_HOP: u32 = 0xffff_ffff;

/// What a traced node answered: the route there and back, with the SNR each
/// hop was heard at, in dB, `None` where it could not be told.
#[derive(Debug, Clone, PartialEq)]
pub struct TracerouteAnswer {
    /// The id of the traceroute sent.
    pub request_id: u32,
    /// The node traced, which is who answered.
    pub from: u32,
    /// The relays on the way there, [`UNKNOWN_HOP`] for one not told.
    pub route: Vec<u32>,
    /// One for each hop there: to each relay, and to the node traced.
    pub snr_towards: Vec<Option<f32>>,
    /// The relays on the way back.
    pub route_back: Vec<u32>,
    /// One for each hop back; one more than relays when the answer was heard
    /// arriving home, which it was, since here it is.
    pub snr_back: Vec<Option<f32>>,
}

/// What the firmware puts for an SNR it could not tell: INT8_MIN.
const UNKNOWN_SNR: i32 = -128;

fn snrs(scaled: &[i32]) -> Vec<Option<f32>> {
    scaled
        .iter()
        // Scaled by four, which an i8 range fits in an f32 exactly.
        .map(|&s| (s != UNKNOWN_SNR).then(|| s as f32 / 4.0))
        .collect()
}

/// The node number that stands for everyone: what a channel message is sent to.
pub const BROADCAST: u32 = 0xffff_ffff;

/// A text message, as heard.
#[derive(Debug, Clone, PartialEq)]
pub struct TextMessage {
    /// The packet's id, which with `from` names it across the mesh.
    pub packet_id: u32,
    pub from: u32,
    /// [`BROADCAST`] for a channel message, a node for a direct one.
    pub to: u32,
    /// The channel's slot on the radio.
    pub channel: u32,
    pub text: String,
    /// When it was received, in seconds since the epoch.
    pub at: u32,
    /// The packet it answers, if any.
    pub reply_id: Option<u32>,
    /// Who sent it, by name, when that is all there is to go on: a MeshCore
    /// channel message carries no sender, only the name the sending client
    /// wrote into its text.
    pub sender_name: Option<String>,
    /// Whether it is a reaction (an emoji) to `reply_id` rather than a message.
    pub emoji: bool,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hops_away: Option<u32>,
    pub via_mqtt: bool,
    /// When its sender says it sent it, by its own clock: on MeshCore, where
    /// reactions name a message by it.
    pub sent_at: Option<u32>,
    /// Who wrote it, when that is not who sent it: a MeshCore room's post,
    /// which the room sends, by its author's node number.
    pub author: Option<u32>,
}

/// A packet as the radio heard it.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    /// The packet's id, which with `from` names it across the mesh.
    pub packet_id: u32,
    pub from: u32,
    /// [`BROADCAST`] for everyone.
    pub to: u32,
    /// The channel's slot on the radio that heard it.
    pub channel: u32,
    /// What it carries, as the protobufs number it; none when the radio could
    /// not decrypt it, and then `payload` is still encrypted.
    pub portnum: Option<i32>,
    pub payload: Vec<u8>,
    /// The packet it answers, for an answer.
    pub request_id: Option<u32>,
    /// The packet it replies to, for a message that does.
    pub reply_id: Option<u32>,
    /// Whether whoever sent it asked for an answer.
    pub want_response: bool,
    /// Whether it was encrypted to the radio's own key, not a channel's.
    pub pki: bool,
    /// When it was received, in seconds since the epoch.
    pub at: u32,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hop_limit: u32,
    /// How many hops it was sent with; 0 when the firmware that sent it is
    /// too old to say.
    pub hop_start: u32,
    /// The last byte of the node that relayed it last, when the firmware says.
    pub relay_node: Option<u32>,
    pub via_mqtt: bool,
}

impl Heard {
    fn from(packet: &pb::MeshPacket, now: u32) -> Self {
        let (portnum, payload, request_id, reply_id, want_response) = match &packet.payload_variant
        {
            Some(mesh_packet::PayloadVariant::Decoded(data)) => (
                Some(data.portnum),
                data.payload.clone(),
                (data.request_id != 0).then_some(data.request_id),
                (data.reply_id != 0).then_some(data.reply_id),
                data.want_response,
            ),
            Some(mesh_packet::PayloadVariant::Encrypted(bytes)) => {
                (None, bytes.clone(), None, None, false)
            }
            None => (None, Vec::new(), None, None, false),
        };
        let reception = !packet.via_mqtt;
        Self {
            packet_id: packet.id,
            from: packet.from,
            to: packet.to,
            channel: packet.channel,
            portnum,
            payload,
            request_id,
            reply_id,
            want_response,
            pki: packet.pki_encrypted,
            at: packet.rx_time.filter(|t| *t > 0).unwrap_or(now),
            snr: reception.then_some(packet.rx_snr).filter(|s| *s != 0.0),
            rssi: packet.rx_rssi.filter(|r| reception && *r != 0),
            hop_limit: packet.hop_limit,
            hop_start: packet.hop_start,
            relay_node: (packet.relay_node != 0).then_some(packet.relay_node),
            via_mqtt: packet.via_mqtt,
        }
    }
}

/// What became of a packet sent with want_ack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    /// The id of the packet sent.
    pub packet_id: u32,
    /// Who answered: the destination when it was delivered, the radio itself
    /// when it heard it relayed or gave up.
    pub from: u32,
    /// Why it failed, as the protobufs name it; none if it did not.
    pub error: Option<String>,
}

/// Everything known about the radio and the mesh.
#[derive(Debug, Clone, Default)]
pub struct MeshState {
    pub link: LinkState,
    /// When the link came to be as it is, in seconds since the epoch; none
    /// until it first changes.
    pub link_since: Option<u32>,
    pub local: Option<LocalNode>,
    pub channels: BTreeMap<u32, Channel>,
    pub nodes: BTreeMap<u32, Node>,
    /// The radio's own settings, as it sent them on syncing.
    pub settings: Settings,
    /// The nodes the radio names while it is read: those it has, once it is
    /// read in full.
    pub named: BTreeSet<u32>,
    /// Whether the radio is being read again, quietly, its link as it was:
    /// what changes is told once it has said it all.
    pub rereading: bool,
    /// A MeshCore radio's own settings, as it last said them; none for a
    /// Meshtastic one, whose are in `settings`.
    pub meshcore: Option<crate::meshcore::Settings>,
    /// A MeshCore radio's channels' slots and secrets, their names being in
    /// `channels`; none for a Meshtastic one.
    pub meshcore_channels: Option<crate::meshcore::Channels>,
}

/// The radio's settings, whole and as the protobufs carry them: what changing
/// them starts from, so that whatever is not changed is sent back as it was.
///
/// They hold secrets (channel keys, the Wi-Fi password, the private key), and
/// live in memory only: nothing stores them, and nothing serves them as they
/// are.
#[derive(Clone, Default)]
pub struct Settings {
    pub owner: Option<pb::User>,
    pub config: pb::LocalConfig,
    /// Each module's configuration, by the name its part of `ModuleConfig`
    /// has in the protobufs (`mqtt`, `telemetry`…): whichever modules the
    /// radio's firmware has, these protobufs knowing them or not.
    pub modules: BTreeMap<String, pb::ModuleConfig>,
    pub channels: BTreeMap<u32, pb::Channel>,
    /// What the radio says it has: Wi-Fi, Bluetooth, Ethernet, the modules
    /// built in. `None` until it says.
    pub metadata: Option<pb::DeviceMetadata>,
}

impl std::fmt::Debug for Settings {
    // Never printed whole, secrets and all, into a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("owner", &self.owner.as_ref().map(|o| &o.long_name))
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

impl Settings {
    /// Takes in one module's configuration.
    fn keep_module(&mut self, module: &pb::ModuleConfig) {
        let Some(descriptor) =
            meshctl_proto::descriptors().get_message_by_name("meshtastic.ModuleConfig")
        else {
            return;
        };
        let Ok(dynamic) =
            prost_reflect::DynamicMessage::decode(descriptor, module.encode_to_vec().as_slice())
        else {
            return;
        };
        // The one part that is set says which module it is.
        if let Some((field, _)) = dynamic.fields().next() {
            self.modules.insert(field.name().to_owned(), module.clone());
        }
    }

    /// Takes in one part of the configuration.
    fn keep(&mut self, config: &pb::Config) {
        use config::PayloadVariant as C;
        let local = &mut self.config;
        match config.payload_variant.clone() {
            Some(C::Device(c)) => local.device = Some(c),
            Some(C::Position(c)) => local.position = Some(c),
            Some(C::Power(c)) => local.power = Some(c),
            Some(C::Network(c)) => local.network = Some(c),
            Some(C::Display(c)) => local.display = Some(c),
            Some(C::Lora(c)) => local.lora = Some(c),
            Some(C::Bluetooth(c)) => local.bluetooth = Some(c),
            Some(C::Security(c)) => local.security = Some(c),
            _ => {}
        }
    }
}

/// A status as a node says it: none for nothing at all.
pub fn status_of(status: &str) -> Option<String> {
    let status = status.trim();
    (!status.is_empty()).then(|| status.to_owned())
}

impl MeshState {
    /// The link as it goes to `link`, and which nodes the radio has once it
    /// has said them all: until then, as known before.
    pub(crate) fn link_to(&mut self, link: LinkState) {
        // A reading again is over, whatever the link does: a sync reads it
        // all anyway.
        self.rereading = false;
        match link {
            LinkState::Syncing => self.named.clear(),
            LinkState::Ready if self.link == LinkState::Syncing => self.read_through(),
            _ => {}
        }
        self.link = link;
    }

    /// Whether the radio is saying all it has: on connecting, or read again.
    fn reading(&self) -> bool {
        self.link == LinkState::Syncing || self.rereading
    }

    /// Which nodes the radio has, now that it has named them all.
    pub(crate) fn read_through(&mut self) {
        let named = std::mem::take(&mut self.named);
        for node in self.nodes.values_mut() {
            node.on_radio = Some(named.contains(&node.num));
        }
    }

    /// The radio about to be read again, its link as it is.
    pub(crate) fn reread(&mut self) {
        self.rereading = true;
        self.named.clear();
    }

    /// Read again in full: what changed, told all at once, as on syncing.
    pub(crate) fn reread_done(&mut self) -> Vec<Event> {
        if !self.rereading {
            return Vec::new();
        }
        self.rereading = false;
        self.read_through();
        vec![Event::Synced]
    }

    /// Not read again in full: as it was known.
    pub(crate) fn reread_failed(&mut self) {
        self.rereading = false;
        self.named.clear();
    }

    /// Takes in a message from the radio received at `now` (seconds since the
    /// epoch) and returns what changed.
    ///
    /// While syncing, nodes change without an event each: the radio sends its
    /// whole list at once, and [`Event::Synced`] at the end says it all.
    pub fn apply(&mut self, message: pb::FromRadio, now: u32) -> Vec<Event> {
        use from_radio::PayloadVariant as V;

        let quiet = self.reading();
        let changed = match message.payload_variant {
            Some(V::MyInfo(info)) => {
                // Another node behind the same radio, as when its owner swaps
                // hardware: its description and channels are its own, but the
                // nodes heard so far are still the mesh around, and are kept.
                if self
                    .local
                    .as_ref()
                    .is_some_and(|l| l.num != info.my_node_num)
                {
                    self.local = None;
                    self.channels.clear();
                    self.settings = Settings::default();
                }
                self.local.get_or_insert_with(LocalNode::default).num = info.my_node_num;
                None
            }
            Some(V::Metadata(metadata)) => {
                let local = self.local.get_or_insert_with(LocalNode::default);
                local.firmware_version = Some(metadata.firmware_version.clone());
                local.hw_model = Some(hw_model_name(metadata.hw_model));
                self.settings.metadata = Some(metadata);
                None
            }
            Some(V::Config(config)) => {
                self.settings.keep(&config);
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
            Some(V::ModuleConfig(module)) => {
                self.settings.keep_module(&module);
                // The radio's own status, as its settings have it.
                match (&module.payload_variant, &self.local) {
                    (
                        Some(pb::module_config::PayloadVariant::Statusmessage(status)),
                        Some(local),
                    ) => self.nodes.get_mut(&local.num).map(|node| {
                        node.status = status_of(&status.node_status);
                        node.status_at = None;
                        node.clone()
                    }),
                    _ => None,
                }
            }
            Some(V::Channel(channel)) => {
                let index = u32::try_from(channel.index).unwrap_or_default();
                let role = enum_name(channel.role, |v| {
                    pb::channel::Role::try_from(v).ok().map(|r| r.as_str_name())
                });
                self.settings.channels.insert(index, channel.clone());
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
            Some(V::NodeInfo(info)) => {
                if self.local.as_ref().is_some_and(|l| l.num == info.num) {
                    self.settings.owner.clone_from(&info.user);
                }
                Some(self.node_info(info))
            }
            Some(V::Packet(packet)) => {
                // What the packet says counts even while syncing: a radio, or a
                // tasticsplitter, replays the messages a client missed then.
                let heard = Event::Packet(Heard::from(&packet, now));
                let said = said(&packet, now);
                let node = self.packet(packet, now);
                let mut events = vec![heard];
                if !quiet {
                    events.push(Event::Node(node));
                }
                events.extend(said);
                return events;
            }
            _ => None,
        };

        match changed {
            Some(node) if !quiet => vec![Event::Node(node)],
            _ => Vec::new(),
        }
    }

    fn node_info(&mut self, info: pb::NodeInfo) -> Node {
        let syncing = self.reading();
        let node = self
            .nodes
            .entry(info.num)
            .or_insert_with(|| Node::new(info.num));
        // In the radio's list, as it says.
        if syncing {
            self.named.insert(info.num);
        } else {
            node.on_radio = Some(true);
        }
        if let Some(user) = info.user {
            node.user = Some(user_from(user));
        }
        if let Some(position) = info.position.and_then(position_from) {
            node.position = Some(position);
        }
        if let Some(metrics) = info.device_metrics {
            node.battery_level = metrics.battery_level.or(node.battery_level);
            node.voltage = metrics.voltage.or(node.voltage);
            node.channel_utilization = metrics.channel_utilization.or(node.channel_utilization);
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
        let syncing = self.reading();
        let node = self
            .nodes
            .entry(packet.from)
            .or_insert_with(|| Node::new(packet.from));
        // Heard, the radio keeps it in its list too.
        if syncing {
            self.named.insert(packet.from);
        } else {
            node.on_radio = Some(true);
        }

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
                Ok(PortNum::NodeStatusApp) => {
                    if let Ok(status) = pb::StatusMessage::decode(data.payload.as_slice()) {
                        node.status = status_of(&status.status);
                        node.status_at = node.last_heard;
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
                        node.channel_utilization =
                            metrics.channel_utilization.or(node.channel_utilization);
                    }
                }
                _ => {}
            }
        }

        node.clone()
    }
}

/// A message, or the answer to one, if that is what the packet carries.
fn said(packet: &pb::MeshPacket, now: u32) -> Option<Event> {
    let Some(mesh_packet::PayloadVariant::Decoded(data)) = &packet.payload_variant else {
        return None;
    };
    match PortNum::try_from(data.portnum) {
        Ok(PortNum::TextMessageApp) => {
            let reception = !packet.via_mqtt;
            Some(Event::Message(TextMessage {
                packet_id: packet.id,
                from: packet.from,
                to: packet.to,
                channel: packet.channel,
                text: String::from_utf8_lossy(&data.payload).into_owned(),
                at: packet.rx_time.filter(|t| *t > 0).unwrap_or(now),
                reply_id: (data.reply_id != 0).then_some(data.reply_id),
                sender_name: None,
                emoji: data.emoji != 0,
                snr: reception.then_some(packet.rx_snr).filter(|s| *s != 0.0),
                rssi: packet.rx_rssi.filter(|_| reception),
                hops_away: (reception && packet.hop_start > 0)
                    .then(|| packet.hop_start.saturating_sub(packet.hop_limit)),
                via_mqtt: packet.via_mqtt,
                sent_at: None,
                author: None,
            }))
        }
        Ok(PortNum::AdminApp) if data.request_id != 0 => {
            let admin = pb::AdminMessage::decode(data.payload.as_slice()).ok()?;
            Some(Event::AdminAnswer {
                request_id: data.request_id,
                passkey: admin.session_passkey,
            })
        }
        // Only answers: a request to us the firmware answers on its own.
        Ok(PortNum::TracerouteApp) if data.request_id != 0 => {
            let discovery = pb::RouteDiscovery::decode(data.payload.as_slice()).ok()?;
            Some(Event::Traceroute(TracerouteAnswer {
                request_id: data.request_id,
                from: packet.from,
                route: discovery.route,
                snr_towards: snrs(&discovery.snr_towards),
                route_back: discovery.route_back,
                snr_back: snrs(&discovery.snr_back),
            }))
        }
        Ok(PortNum::RoutingApp) if data.request_id != 0 => {
            let routing = pb::Routing::decode(data.payload.as_slice()).ok()?;
            let error = match routing.variant {
                Some(pb::routing::Variant::ErrorReason(reason)) => reason,
                // A route request or reply is not an answer to anything sent.
                _ => return None,
            };
            Some(Event::Ack(Ack {
                packet_id: data.request_id,
                from: packet.from,
                error: (error != pb::routing::Error::None as i32).then(|| {
                    enum_name(error, |v| {
                        pb::routing::Error::try_from(v)
                            .ok()
                            .map(|e| e.as_str_name())
                    })
                }),
            }))
        }
        _ => None,
    }
}

pub(crate) fn user_from(user: pb::User) -> User {
    User {
        long_name: user.long_name,
        short_name: user.short_name,
        hw_model: hw_model_name(user.hw_model),
        role: role_name(user.role),
        public_key: user.public_key,
        licensed: user.is_licensed,
        unmessagable: user.is_unmessagable.unwrap_or(false),
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
        radius: radius(position.precision_bits),
        source: match pb::position::LocSource::try_from(position.location_source) {
            Ok(pb::position::LocSource::LocManual) => Some(PositionSource::Manual),
            Ok(pb::position::LocSource::LocInternal) => Some(PositionSource::Internal),
            Ok(pb::position::LocSource::LocExternal) => Some(PositionSource::External),
            _ => None,
        },
    })
}

/// How far, in metres, a position shared rounded may be from where the node
/// is: half the side of the cell its precision rounds to, as the official
/// apps tell it. None when it is exact (32 bits) or does not say (0).
fn radius(precision_bits: u32) -> Option<u32> {
    if !(1..32).contains(&precision_bits) {
        return None;
    }
    // A degree of latitude is about 111 320 m; coordinates go by 1e-7 degrees.
    let side = f64::from(1_u32 << (32 - precision_bits)) * 1e-7 * 111_320.0;
    // Under 1 m and over 3000 km cannot happen with 1 to 31 bits.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((side / 2.0).round() as u32)
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
        assert!(matches!(events.first(), Some(Event::Packet(h)) if h.from == 0x1234_5678));
        assert_eq!(events.get(1), Some(&Event::Node(node.clone())));
    }

    #[test]
    fn a_rounded_position_says_how_far_off_it_may_be() {
        // As the official apps tell them.
        assert_eq!(radius(13), Some(2918));
        assert_eq!(radius(16), Some(365));
        assert_eq!(radius(32), None);
        assert_eq!(radius(0), None);
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
    fn which_nodes_the_radio_has_is_known_once_it_is_read() {
        // Kept from before: 5, which the radio no longer has, and 7.
        let mut state = MeshState {
            link: LinkState::Disconnected,
            nodes: [5, 7].map(|num| (num, Node::new(num))).into(),
            ..Default::default()
        };
        state.link_to(LinkState::Syncing);
        state.apply(
            from_radio(V::NodeInfo(pb::NodeInfo {
                num: 7,
                ..Default::default()
            })),
            1,
        );
        state.apply(
            from_radio(V::NodeInfo(pb::NodeInfo {
                num: 9,
                ..Default::default()
            })),
            1,
        );
        // Not known while it is read.
        assert!(state.nodes.values().all(|n| n.on_radio.is_none()));
        state.link_to(LinkState::Ready);
        let on_radio = |state: &MeshState, num| state.nodes[&num].on_radio;
        assert_eq!(
            [5, 7, 9].map(|num| on_radio(&state, num)),
            [Some(false), Some(true), Some(true)]
        );
        // Heard again, the radio keeps it once more.
        state.apply(
            from_radio(V::Packet(packet(5, PortNum::TextMessageApp, vec![]))),
            2,
        );
        assert_eq!(on_radio(&state, 5), Some(true));
        // A drop and a read cut short leave it as it was known.
        state.link_to(LinkState::Disconnected);
        state.link_to(LinkState::Syncing);
        state.link_to(LinkState::Disconnected);
        assert_eq!(on_radio(&state, 9), Some(true));
    }

    #[test]
    fn a_node_says_its_status_and_the_radio_its_own() {
        let mut state = ready();
        state.local = Some(LocalNode {
            num: 1,
            ..Default::default()
        });
        let says = |text: &str| {
            pb::StatusMessage {
                status: text.into(),
            }
            .encode_to_vec()
        };
        state.apply(
            from_radio(V::Packet(packet(
                7,
                PortNum::NodeStatusApp,
                says(" En ruta "),
            ))),
            50,
        );
        assert_eq!(state.nodes[&7].status.as_deref(), Some("En ruta"));
        assert_eq!(state.nodes[&7].status_at, Some(50));
        // Nothing said is no status.
        state.apply(
            from_radio(V::Packet(packet(7, PortNum::NodeStatusApp, says("")))),
            60,
        );
        assert_eq!(state.nodes[&7].status, None);
        // The radio's own, from its settings.
        state.nodes.insert(1, Node::new(1));
        state.apply(
            from_radio(V::ModuleConfig(pb::ModuleConfig {
                payload_variant: Some(pb::module_config::PayloadVariant::Statusmessage(
                    pb::module_config::StatusMessageConfig {
                        node_status: "En casa".into(),
                    },
                )),
            })),
            70,
        );
        assert_eq!(state.nodes[&1].status.as_deref(), Some("En casa"));
        assert_eq!(state.nodes[&1].status_at, None);
    }

    #[test]
    fn another_node_behind_the_radio_keeps_the_mesh_heard() {
        let mut state = ready();
        state.apply(
            from_radio(V::MyInfo(pb::MyNodeInfo {
                my_node_num: 1,
                ..Default::default()
            })),
            1,
        );
        state.apply(
            from_radio(V::Metadata(pb::DeviceMetadata {
                firmware_version: "2.7.0".into(),
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
        assert!(state.nodes.contains_key(&7));
        let local = state.local.unwrap();
        assert_eq!(local.num, 2);
        // What the old node said of itself is not the new one's.
        assert_eq!(local.firmware_version, None);
    }

    #[test]
    fn a_text_message_is_heard() {
        let mut state = ready();
        let mut sent = packet(7, PortNum::TextMessageApp, "hola".into());
        sent.id = 42;
        sent.channel = 4;
        if let Some(mesh_packet::PayloadVariant::Decoded(data)) = &mut sent.payload_variant {
            data.reply_id = 41;
        }
        let events = state.apply(from_radio(V::Packet(sent)), 100);
        let Some(Event::Message(message)) = events.last() else {
            panic!("a message, after the node");
        };
        assert_eq!(message.packet_id, 42);
        assert_eq!(message.text, "hola");
        assert_eq!(message.to, BROADCAST);
        assert_eq!(message.channel, 4);
        assert_eq!(message.reply_id, Some(41));
        assert_eq!(message.hops_away, Some(2));
        assert!(matches!(
            events.as_slice(),
            [Event::Packet(_), Event::Node(_), Event::Message(_)]
        ));
    }

    #[test]
    fn a_message_counts_while_syncing() {
        let mut state = MeshState {
            link: LinkState::Syncing,
            ..Default::default()
        };
        let events = state.apply(
            from_radio(V::Packet(packet(7, PortNum::TextMessageApp, "hola".into()))),
            1,
        );
        assert!(matches!(
            events.as_slice(),
            [Event::Packet(_), Event::Message(_)]
        ));
    }

    fn routing(from: u32, request_id: u32, error: pb::routing::Error) -> pb::MeshPacket {
        let body = pb::Routing {
            variant: Some(pb::routing::Variant::ErrorReason(error as i32)),
            ..Default::default()
        };
        let mut answer = packet(from, PortNum::RoutingApp, body.encode_to_vec());
        if let Some(mesh_packet::PayloadVariant::Decoded(data)) = &mut answer.payload_variant {
            data.request_id = request_id;
        }
        answer
    }

    #[test]
    fn answers_to_what_was_sent_are_heard() {
        let mut state = ready();
        let events = state.apply(
            from_radio(V::Packet(routing(9, 42, pb::routing::Error::None))),
            1,
        );
        assert_eq!(
            events.last(),
            Some(&Event::Ack(Ack {
                packet_id: 42,
                from: 9,
                error: None
            }))
        );
        let events = state.apply(
            from_radio(V::Packet(routing(1, 43, pb::routing::Error::MaxRetransmit))),
            1,
        );
        let Some(Event::Ack(ack)) = events.last() else {
            panic!("an ack");
        };
        assert_eq!(ack.error.as_deref(), Some("MAX_RETRANSMIT"));
    }

    #[test]
    fn a_traceroute_answer_is_heard() {
        let mut state = ready();
        let discovery = pb::RouteDiscovery {
            route: vec![5, UNKNOWN_HOP],
            snr_towards: vec![24, -128, 10],
            route_back: vec![5],
            snr_back: vec![-6, 40],
        };
        let mut answer = packet(9, PortNum::TracerouteApp, discovery.encode_to_vec());
        if let Some(mesh_packet::PayloadVariant::Decoded(data)) = &mut answer.payload_variant {
            data.request_id = 42;
        }
        let events = state.apply(from_radio(V::Packet(answer)), 1);
        let Some(Event::Traceroute(traced)) = events.last() else {
            panic!("a traceroute answer");
        };
        assert_eq!(traced.request_id, 42);
        assert_eq!(traced.from, 9);
        assert_eq!(traced.route, [5, UNKNOWN_HOP]);
        assert_eq!(traced.snr_towards, [Some(6.0), None, Some(2.5)]);
        assert_eq!(traced.snr_back, [Some(-1.5), Some(10.0)]);
    }

    #[test]
    fn a_traceroute_request_is_not_an_answer() {
        let mut state = ready();
        let request = packet(
            9,
            PortNum::TracerouteApp,
            pb::RouteDiscovery::default().encode_to_vec(),
        );
        let events = state.apply(from_radio(V::Packet(request)), 1);
        assert!(!events.iter().any(|e| matches!(e, Event::Traceroute(_))));
    }

    #[test]
    fn each_module_is_kept_by_its_name() {
        let mut state = ready();
        let telemetry = pb::ModuleConfig {
            payload_variant: Some(pb::module_config::PayloadVariant::Telemetry(
                pb::module_config::TelemetryConfig {
                    device_update_interval: 900,
                    ..Default::default()
                },
            )),
        };
        state.apply(from_radio(V::ModuleConfig(telemetry.clone())), 1);
        assert_eq!(state.settings.modules.get("telemetry"), Some(&telemetry));
        assert_eq!(state.settings.modules.len(), 1);
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
