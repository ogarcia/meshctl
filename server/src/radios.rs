// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios meshctl knows of. Each one has a mesh, started from what was
//! stored of it and kept stored as it changes; an enabled one also has a link
//! that keeps that mesh current. They are started, restarted and stopped as
//! radios are added, changed and removed.

use crate::store::{NewMessageRow, RadioRecord, Store, now};
use anyhow::Result;
use meshctl_api::{
    BADGE_COLORS, Badge, BadgeContent, ChannelSecret, Conversation, ErrorCode, FixedPosition, Hop,
    LinkContents, LinkState, MapView, Message, MessageId, MessageStatus, NewMessage, NewRequest,
    NodeAccess, NodeCheck, NodeLatest, NodeLogin, NodeSettings, NodeSettingsChange, NodeTelemetry,
    Pace, Packet, PacketFilter, Peer, PositionShare, Preferences, Radio, RadioId, RadioKind,
    RadioSettings, Request, RequestKind, SectionSecret, SettingsApplied, SettingsSection,
    SightingId, Status, TelemetryPoint, TelemetrySeries, Waits,
};
use meshctl_core::link::Transport;
use meshctl_core::mesh::{self, MeshState};
use meshctl_core::radio::{AdminError, Ask, Mesh, OutgoingText, Protocol, SendError};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// The longest name a radio may have, in characters.
const NAME_MAX: usize = 64;

/// How many events a slow follower may fall behind before it misses some.
const EVENT_BACKLOG: usize = 4096;

/// How often what is kept for longer than it is kept is cleaned away.
const CLEANING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

const DAY: i64 = 24 * 3600;

/// What changed, across every radio.
#[derive(Debug, Clone)]
pub enum Change {
    /// A radio was added, changed or removed.
    Radios,
    /// Something changed on one radio: its link, or a node. Messages and
    /// their answers come as [`Change::Message`] and [`Change::MessageStatus`]
    /// instead, once kept.
    Mesh(RadioId, mesh::Event),
    /// A message heard or sent, as kept.
    Message(RadioId, Message),
    /// A waypoint heard or shared, new or changed, as kept.
    Waypoint(RadioId, meshctl_api::waypoints::Waypoint),
    /// A waypoint gone: deleted, expired, or forgotten in meshctl.
    WaypointGone(RadioId, u32),
    /// A node gone in or out of a fence watched, as kept: by whom, and the
    /// waypoint's emoji and name.
    FenceCrossed {
        radio: RadioId,
        waypoint: u32,
        crossing: meshctl_api::waypoints::FenceCrossing,
        asked: bool,
        who: String,
        icon: String,
        name: String,
    },
    /// A login to a room sent, and how long its answer is waited for.
    LoginWaiting {
        radio: RadioId,
        node: u32,
        wait_ms: u32,
    },
    /// Another copy of a message heard.
    MessageHeard {
        radio: RadioId,
        id: MessageId,
        heard: u32,
        repeaters: u32,
        scope: Option<String>,
    },
    /// What became of a message sent.
    MessageStatus {
        radio: RadioId,
        id: MessageId,
        status: MessageStatus,
        error: Option<String>,
    },
    /// A packet a radio heard, as kept.
    Packet(RadioId, Packet),
    /// Something a node was asked for, or what became of it.
    Request(RadioId, Request),
    /// The interface's preferences changed.
    Preferences(Preferences),
    /// All kept of a node was forgotten, as asked: what was asked of it, the
    /// routes traced to it, its conversation and what it sent.
    NodeForgotten(RadioId, u32),
    /// Messages forgotten, as asked: one, and the reactions to it.
    MessagesForgotten(RadioId, Vec<MessageId>),
    /// A conversation's messages forgotten, as asked: a channel emptied, or
    /// a conversation with a node gone.
    ConversationForgotten(RadioId, Peer),
    /// A message to notify of.
    Notification(meshctl_api::Notification),
    /// A conversation read further, up to a message.
    ConversationMuted {
        radio: RadioId,
        peer: Peer,
        muted: bool,
    },
    ConversationRead {
        radio: RadioId,
        peer: Peer,
        up_to: MessageId,
    },
    /// What was kept for longer than it is kept, or as asked, cleaned away.
    Cleaned(meshctl_api::Cleaned),
    /// How long what meshctl keeps is kept changed.
    Retention(meshctl_api::Retention),
    /// Events of a radio were lost on their way here.
    Lagged,
}

/// The turn to log in to a node before a request.
type LoginGate = Arc<tokio::sync::Mutex<()>>;

/// How a login made before a request failed, and when.
type LoginFailed = (std::time::Instant, LoginFailure);

/// What a radio was let in to a room as, and when.
type LoginHeld = (NodeAccess, i64);

/// Why a radio did not log in to a node: a room, a repeater or a sensor.
#[derive(Debug)]
pub enum LoginFailure {
    NoSuchRadio,
    /// Not a node the radio knows that takes logins.
    NoLogin,
    /// A node the radio no longer has: meshctl alone keeps it, as heard
    /// before.
    NotOnRadio,
    NotConnected,
    /// The room said no to the password.
    WrongPassword,
    /// Nothing came back: the room is out of reach, or it does not take
    /// the password and lets nobody in to read.
    NoAnswer,
    /// Logged in to as less than its admin, for what only its admin may do.
    NotAdmin,
    /// Not a command a console takes: empty, or longer than a message.
    Invalid,
    /// The radio would not send it, and said why.
    Refused(String),
    Failed(anyhow::Error),
}

impl LoginFailure {
    /// The same failure again, for another that waited for it.
    fn again(&self) -> Self {
        match self {
            Self::NoSuchRadio => Self::NoSuchRadio,
            Self::NoLogin => Self::NoLogin,
            Self::NotOnRadio => Self::NotOnRadio,
            Self::NotConnected => Self::NotConnected,
            Self::WrongPassword => Self::WrongPassword,
            Self::NoAnswer => Self::NoAnswer,
            Self::NotAdmin => Self::NotAdmin,
            Self::Invalid => Self::Invalid,
            Self::Refused(why) => Self::Refused(why.clone()),
            Self::Failed(err) => Self::Failed(anyhow::anyhow!("{err:#}")),
        }
    }
}

impl From<anyhow::Error> for LoginFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// Why a message was not sent.
#[derive(Debug)]
pub enum SendFailure {
    NoSuchRadio,
    NotConnected,
    Invalid,
    /// No position to send: the radio does not know where it is, or the
    /// channel it would go on shares none.
    NotShared,
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for SendFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// How long what a radio said of its channel's business holds.
const BUSY_FRESH: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How often requests waited for too long are looked for.
const EXPIRY_TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// How far back a sensor's kept readings are asked for when not said: a
/// day.
const SERIES_SECONDS: u32 = 24 * 3600;

/// How long a request is waited for before it is given up on.
const REQUEST_TIMEOUT: i64 = 120;

/// How far away the repeaters asked for their regions may be, in hops: past
/// that, what they serve matters little to what the radio floods.
const REGIONS_HOPS: u32 = 2;
/// How many are asked in one search at most.
const REGIONS_MOST: usize = 12;
/// How long since a repeater last said its regions before it is asked again
/// in a search, in seconds.
const REGIONS_FRESH: i64 = 600;
/// How long each is waited for.
const REGIONS_WAIT: Duration = Duration::from_secs(15);
/// The pause between one and the next.
const REGIONS_PAUSE: Duration = Duration::from_secs(3);

/// How far back, in seconds, nodes heard with the same public key count as
/// sharing it: a week, so that one long gone does not.
const SHARED_KEY_WINDOW: i64 = 7 * 24 * 3600;

/// How many of a node's latest node infos are looked into for more than
/// one key or hardware model.
const NODE_INFOS_CHECKED: u32 = 50;

/// The most points a chart's series has: more than a chart's width shows,
/// few enough to send at once.
const CHART_POINTS: usize = 300;

/// How many of a node's latest telemetry packets are looked into for the
/// latest of each kind: one it sends seldom, as power, may be further back
/// than its device telemetry goes.
const TELEMETRY_LOOKED_INTO: u32 = 500;

/// Why a request was not sent.
#[derive(Debug)]
pub enum AskFailure {
    NoSuchRadio,
    NotConnected,
    /// Not something meshctl does with this kind of radio yet.
    Unsupported,
    /// Everyone, or the radio itself.
    InvalidNode,
    /// The radio has no such channel, or it is disabled.
    InvalidChannel,
    /// The same was asked of the same node, and is still waiting.
    Pending,
    /// A node the radio knows no path to, for a request that goes only
    /// direct.
    NoPath,
    /// The radio sent the like too recently, and would drop it: so many
    /// seconds more to wait.
    TooSoon {
        wait: i64,
    },
    /// It needs a login, which the radio could not make.
    Login(LoginFailure),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for AskFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// Why nothing was added from a link.
#[derive(Debug)]
pub enum LinkFailure {
    /// Not a link meshctl reads.
    Invalid,
    /// The radio has not the free slots its channels take.
    NoFreeSlots,
    Settings(SettingsFailure),
    Admin(AdminError),
}

/// Why a radio's settings were not shown or changed.
#[derive(Debug)]
pub enum SettingsFailure {
    NoSuchRadio,
    /// Not connected, or not yet in sync: its settings are known only then.
    NotConnected,
    /// Which setting it is that the radio cannot have.
    Invalid(String),
    /// A channel the very same as another the radio has.
    Duplicate,
    /// A position more precise than a public channel takes.
    PublicPrecision,
    NoAnswer,
    /// Not something meshctl does with this kind of radio yet.
    Unsupported,
    /// What meshctl keeps could not be read.
    Failed(anyhow::Error),
    /// A node the radio does not know of.
    UnknownNode,
    /// A node that does not take the radio's key as an admin key.
    NotAdmin,
    /// The node, or the radio on its way, would not, and said why, as the
    /// protobufs name it.
    Refused(String),
}

/// The routes a traceroute answer tells, each from its first node to its last:
/// from the radio to the node traced, and back. Each hop with the SNR it was
/// heard at, which the firmware gives one for each hop, in order.
pub fn routes(origin: u32, answer: &mesh::TracerouteAnswer) -> (Vec<Hop>, Vec<Hop>) {
    let path = |first: u32, relays: &[u32], last: u32, snrs: &[Option<f32>]| -> Vec<Hop> {
        let first = Hop::known(Some(first), None);
        let rest = relays
            .iter()
            .copied()
            .chain([last])
            .enumerate()
            .map(|(i, node)| {
                Hop::known(
                    (node != mesh::UNKNOWN_HOP).then_some(node),
                    snrs.get(i).copied().flatten(),
                )
            });
        std::iter::once(first).chain(rest).collect()
    };
    (
        path(origin, &answer.route, answer.from, &answer.snr_towards),
        path(answer.from, &answer.route_back, origin, &answer.snr_back),
    )
}

/// Takes a traceroute's answer as the reply to the request it answers: its
/// routes from the radio, as its own node is now.
async fn answer_traceroute(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    mesh: &Mesh,
    answer: &mesh::TracerouteAnswer,
) -> Result<()> {
    let origin = mesh
        .read(|state| state.local.as_ref().map(|l| l.num))
        .unwrap_or_default();
    let (towards, back) = routes(origin, answer);
    keep_traceroute(
        store,
        changes,
        radio,
        answer.request_id,
        answer.from,
        towards,
        back,
    )
    .await
}

/// Keeps a traceroute's routes as the reply to the request it answers, if it
/// waits for it, and tells of it.
async fn keep_traceroute(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    request_id: u32,
    from: u32,
    towards: Vec<Hop>,
    back: Vec<Hop>,
) -> Result<()> {
    let reply = meshctl_api::RequestReply::Traceroute { towards, back };
    if let Some(request) = store
        .reply_request(radio, request_id, from, &reply, now())
        .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    Ok(())
}

/// A MeshCore traceroute come back, as its event tells it.
struct Traced<'a> {
    hops: &'a [Vec<u8>],
    snrs: &'a [f32],
    last_snr: f32,
}

/// Takes a MeshCore traceroute come back as the answer to the one sent: its
/// hops there and back, each relay by the node it most likely is, as a
/// flood's route tells, the node traced in the middle.
async fn answer_meshcore_trace(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    mesh: &Mesh,
    request_id: u32,
    target: u32,
    traced: &Traced<'_>,
) -> Result<()> {
    let origin = mesh
        .read(|state| state.local.as_ref().map(|l| l.num))
        .unwrap_or_default();
    let (towards, back) = meshcore_trace_routes(mesh, origin, target, traced);
    keep_traceroute(store, changes, radio, request_id, target, towards, back).await
}

/// A MeshCore trace's routes: from the radio through the relays to the node
/// traced, and back the same way; each hop with the SNR it heard the one
/// before it at, the radio with the SNR it heard the last at.
fn meshcore_trace_routes(
    mesh: &Mesh,
    origin: u32,
    target: u32,
    traced: &Traced<'_>,
) -> (Vec<Hop>, Vec<Hop>) {
    let middle = traced.hops.len() / 2;
    let snr = |at: usize| traced.snrs.get(at).copied();
    // The relays, as the way back names them, to the radio: guessed once,
    // the way there the same ones the other way round.
    let back_relays: Vec<&[u8]> = traced.hops[middle + 1..]
        .iter()
        .map(Vec::as_slice)
        .collect();
    let resolved = hops_of(&known_nodes(mesh), &back_relays, true);
    let relay = |hop: &meshctl_api::RouteHop, snr: Option<f32>| Hop {
        node: hop.likely,
        snr,
        hash: Some(hop.hash.clone()),
        guess: hop.likely.is_some()
            && !(hop.nodes.as_slice() == [hop.likely.unwrap_or_default()] && hop.hash.len() >= 4),
    };
    let mut towards = vec![Hop::known(Some(origin), None)];
    towards.extend(
        resolved
            .iter()
            .rev()
            .enumerate()
            .map(|(i, hop)| relay(hop, snr(i))),
    );
    towards.push(Hop::known(Some(target), snr(middle)));
    let mut back = vec![Hop::known(Some(target), None)];
    back.extend(
        resolved
            .iter()
            .enumerate()
            .map(|(i, hop)| relay(hop, snr(middle + 1 + i))),
    );
    back.push(Hop::known(Some(origin), Some(traced.last_snr)));
    (towards, back)
}

/// Keeps a packet a radio heard, tells of it, and takes it as the answer to a
/// request when it is one.
async fn keep_packet(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    heard: &mesh::Heard,
) -> Result<()> {
    let Some(row) = store.add_packet(radio, heard).await? else {
        return Ok(());
    };
    let sighting = row.id;
    let _ = changes.send(Change::Packet(radio, crate::packets::packet(row)));
    if let Some(request_id) = heard.request_id
        && let Some(request) = store
            .answer_request(
                radio,
                request_id,
                heard.from,
                heard.portnum,
                sighting,
                i64::from(heard.at),
            )
            .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    Ok(())
}

/// Keeps a copy of a packet a MeshCore radio heard raw, read, and tells the
/// live view: a new packet, or one heard again, with its copies counted.
async fn keep_raw(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    mesh: &Mesh,
    heard: mesh::RawHeard,
) -> Result<()> {
    use mesh::RawInfo;
    use meshctl_api::PacketKind;
    use serde_json::json;
    let node = |num: u32| format!("!{num:08x}");
    let hash = |byte: u8| format!("{byte:02x}");
    let (kind, from, to, channel, info) = match &heard.info {
        RawInfo::Advert {
            node: num,
            kind,
            name,
            position,
            timestamp,
        } => (
            PacketKind::NodeInfo,
            Some(*num),
            None,
            None,
            json!({
                "node": node(*num),
                "kind": match kind {
                    1 => "companion",
                    2 => "repeater",
                    3 => "room",
                    4 => "sensor",
                    _ => "unknown",
                },
                "name": name,
                "latitude": position.map(|(lat, _)| lat),
                "longitude": position.map(|(_, lon)| lon),
                "clock": timestamp,
            }),
        ),
        RawInfo::ChannelText {
            channel,
            sender,
            text,
            timestamp,
        } => {
            // Who wrote it, as long as one contact alone has that name.
            let from = sender.as_ref().and_then(|name| {
                mesh.read(|state| {
                    let mut named = state
                        .nodes
                        .values()
                        .filter(|n| n.user.as_ref().is_some_and(|u| &u.long_name == name));
                    match (named.next(), named.next()) {
                        (Some(one), None) => Some(one.num),
                        _ => None,
                    }
                })
            });
            (
                PacketKind::Text,
                from,
                None,
                Some(*channel),
                json!({ "channel": channel, "sender": sender, "text": text, "clock": timestamp }),
            )
        }
        RawInfo::Sealed { channel_hash } => (
            PacketKind::Encrypted,
            None,
            None,
            None,
            json!({ "channel_hash": hash(*channel_hash) }),
        ),
        RawInfo::Between { to, from } => (
            // A path found is the mesh's routing; the rest, sealed.
            if heard.payload_type == 0x08 {
                PacketKind::Routing
            } else {
                PacketKind::Encrypted
            },
            None,
            None,
            None,
            json!({ "to": hash(*to), "from": hash(*from) }),
        ),
        RawInfo::Anonymous { to, from } => (
            PacketKind::Encrypted,
            Some(*from),
            None,
            None,
            json!({ "to": hash(*to), "from": node(*from) }),
        ),
        RawInfo::Ack { code } => (
            PacketKind::Routing,
            None,
            None,
            None,
            json!({ "code": format!("{code:08x}") }),
        ),
        // The way it was sent along, each hop by its hash; and the SNR each
        // hop gone through heard it at, its packet's path.
        RawInfo::Trace {
            tag,
            hashes,
            hash_size,
        } => (
            PacketKind::Traceroute,
            None,
            None,
            None,
            json!({
                "tag": format!("{tag:08x}"),
                "hops": hashes
                    .chunks(usize::from(*hash_size).max(1))
                    .map(|hop| hop.iter().map(|b| hash(*b)).collect::<String>())
                    .collect::<Vec<_>>(),
                "snrs": heard
                    .path
                    .iter()
                    .map(|b| f32::from(b.cast_signed()) / 4.0)
                    .collect::<Vec<_>>(),
            }),
        ),
        RawInfo::Other => (PacketKind::Other, None, None, None, json!({})),
    };
    let copy = crate::store::NewMeshcoreCopy {
        key: heard.key,
        payload_type: heard.payload_type,
        kind,
        from,
        to,
        channel,
        info,
        payload_len: heard.payload_len,
        at: i64::from(heard.at),
        snr: heard.snr,
        rssi: heard.rssi,
        route: heard.route,
        scope: match &heard.region {
            Some(region) => Some(region_name(store, radio, mesh, region).await?),
            None => None,
        },
        path: heard.path,
        hash_size: heard.hash_size,
    };
    let packet = store.add_meshcore_copy(radio, &copy).await?;
    let _ = changes.send(Change::Packet(radio, packet));
    Ok(())
}

/// Takes what a node answered with no packet as the answer to its request.
async fn keep_reply(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    mesh: &Mesh,
    reply: &mesh::Reply,
) -> Result<()> {
    let kept = reply_of(mesh, &reply.answer);
    if let Some(request) = store
        .reply_request(radio, reply.request_id, reply.from, &kept, now())
        .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    Ok(())
}

/// A node's answer, as the API tells it: a route's hops with the nodes
/// they may be, as the radio knows them.
fn reply_of(mesh: &Mesh, answer: &mesh::Answer) -> meshctl_api::RequestReply {
    use meshctl_api::RequestReply;
    match answer {
        mesh::Answer::Regions {
            names,
            unscoped,
            clock,
        } => RequestReply::Regions {
            regions: names.clone(),
            unscoped: *unscoped,
            clock: clock.map(i64::from),
        },
        mesh::Answer::Status(status) => RequestReply::Status(meshctl_api::NodeStatus {
            voltage: f32::from(status.battery_mv) / 1000.0,
            queue: u32::from(status.queue),
            noise_floor: i32::from(status.noise_floor),
            last_rssi: i32::from(status.last_rssi),
            last_snr: status.last_snr,
            received: status.received,
            sent: status.sent,
            received_flood: status.received_flood,
            received_direct: status.received_direct,
            sent_flood: status.sent_flood,
            sent_direct: status.sent_direct,
            flood_duplicates: u32::from(status.flood_duplicates),
            direct_duplicates: u32::from(status.direct_duplicates),
            airtime: status.airtime,
            uptime: status.uptime,
            errors: u32::from(status.errors),
            receive_airtime: status.receive_airtime,
            receive_errors: status.receive_errors,
            posted: status.posted.map(u32::from),
            pushed: status.pushed.map(u32::from),
        }),
        mesh::Answer::Telemetry(readings) => RequestReply::Telemetry {
            readings: readings
                .iter()
                .map(|r| meshctl_api::SensorReading {
                    channel: u32::from(r.channel),
                    kind: lpp_name(r.kind),
                    values: r.values.clone(),
                })
                .collect(),
        },
        mesh::Answer::Series {
            seconds,
            clock,
            readings,
        } => RequestReply::Series {
            seconds: *seconds,
            clock: (*clock > 0).then_some(i64::from(*clock)),
            readings: readings
                .iter()
                .map(|r| meshctl_api::SeriesReading {
                    channel: u32::from(r.channel),
                    kind: lpp_name(r.kind),
                    min: r.min,
                    max: r.max,
                    mean: r.mean,
                })
                .collect(),
        },
        mesh::Answer::Neighbours { total, heard } => RequestReply::Neighbors {
            total: u32::from(*total),
            nodes: heard
                .iter()
                .map(|n| meshctl_api::HeardNeighbor {
                    node: n.node,
                    seconds_ago: n.seconds_ago,
                    snr: n.snr,
                })
                .collect(),
        },
        mesh::Answer::Owner(owner) => RequestReply::Owner {
            firmware: owner.firmware.clone(),
            name: owner.name.clone(),
            owner: owner.owner.clone(),
        },
        mesh::Answer::AccessList(listed) => RequestReply::AccessList {
            nodes: listed
                .iter()
                .map(|(node, bits)| meshctl_api::ListedAccess {
                    node: *node,
                    access: match bits & 3 {
                        3 => NodeAccess::Admin,
                        2 => NodeAccess::ReadWrite,
                        _ => NodeAccess::ReadOnly,
                    },
                })
                .collect(),
        },
        mesh::Answer::Ping {
            round_trip_ms,
            snr_there,
            snr_back,
        } => RequestReply::Ping {
            round_trip_ms: *round_trip_ms,
            snr_there: *snr_there,
            snr_back: *snr_back,
        },
        // The way back as a flood came, to the radio; the way there read
        // the other way round for it, and told as it goes.
        mesh::Answer::Route { there, back } => {
            let known = known_nodes(mesh);
            fn slices(hops: &[Vec<u8>]) -> Vec<&[u8]> {
                hops.iter().map(Vec::as_slice).collect()
            }
            let mut towards = slices(there);
            towards.reverse();
            let mut there = hops_of(&known, &towards, true);
            there.reverse();
            RequestReply::Route {
                there,
                back: hops_of(&known, &slices(back), true),
            }
        }
    }
}

/// What a Cayenne LPP type reads, by name; others by their number.
fn lpp_name(kind: u8) -> String {
    match kind {
        0 => "digital_input",
        1 => "digital_output",
        2 => "analog_input",
        3 => "analog_output",
        100 => "generic",
        101 => "illuminance",
        102 => "presence",
        103 => "temperature",
        104 => "humidity",
        113 => "acceleration",
        115 => "pressure",
        116 => "voltage",
        117 => "current",
        118 => "frequency",
        120 => "percentage",
        121 => "altitude",
        125 => "concentration",
        128 => "power",
        130 => "distance",
        131 => "energy",
        132 => "direction",
        133 => "time",
        134 => "rotation",
        135 => "colour",
        136 => "place",
        142 => "switch",
        other => return other.to_string(),
    }
    .to_owned()
}

/// Each node known by its key, as a hop of a route may be; and where the
/// radio is.
type Known = (Vec<(Vec<u8>, crate::routes::Candidate)>, Option<(f64, f64)>);

fn known_nodes(mesh: &Mesh) -> Known {
    use crate::routes::{Candidate, Repeats};
    mesh.read(|state| {
        let at = |n: &mesh::Node| n.position.map(|p| (p.latitude, p.longitude));
        let known = state
            .nodes
            .values()
            .filter_map(|n| {
                let user = n.user.as_ref()?;
                (!user.public_key.is_empty()).then(|| {
                    let candidate = Candidate {
                        num: n.num,
                        at: at(n),
                        repeats: match user.role.as_str() {
                            "REPEATER" => Repeats::Yes,
                            "ROOM" => Repeats::Maybe,
                            _ => Repeats::No,
                        },
                        direct: n.hops_away == Some(0),
                    };
                    (user.public_key.clone(), candidate)
                })
            })
            .collect();
        let radio = state
            .local
            .as_ref()
            .and_then(|l| state.nodes.get(&l.num))
            .and_then(at);
        (known, radio)
    })
}

/// A route's hops, by their hashes, from the far end to the radio: each with
/// the nodes the radio knows whose key starts so, and the one it most
/// likely is, as a flood's route tells; a direct one's, only one a hash
/// alone tells, as it goes on to where it is sent.
fn hops_of(known: &Known, hashes: &[&[u8]], flooded: bool) -> Vec<meshctl_api::RouteHop> {
    let (known, radio) = known;
    let candidates: Vec<Vec<crate::routes::Candidate>> = hashes
        .iter()
        .map(|hash| {
            known
                .iter()
                .filter(|(key, _)| key.starts_with(hash))
                .map(|(_, c)| c.clone())
                .collect()
        })
        .collect();
    let likely = if flooded {
        crate::routes::likely(&candidates, *radio)
    } else {
        candidates
            .iter()
            .map(|c| match c.as_slice() {
                [one] => Some(one.num),
                _ => None,
            })
            .collect()
    };
    hashes
        .iter()
        .zip(candidates)
        .zip(likely)
        .map(|((hash, candidates), likely)| meshctl_api::RouteHop {
            hash: hash.iter().map(|b| format!("{b:02x}")).collect(),
            nodes: candidates.iter().map(|c| c.num).collect(),
            likely,
        })
        .collect()
}

/// Copies heard as the ways they came: each hop by its hash, with the nodes
/// the radio knows whose key starts so, and the one it most likely is.
fn routes_of(mesh: &Mesh, rows: Vec<crate::store::RouteRow>) -> Vec<meshctl_api::MessageRoute> {
    let known = known_nodes(mesh);
    rows.into_iter()
        .map(|row| {
            let size = usize::try_from(row.hash_size).unwrap_or(1).max(1);
            let hashes: Vec<&[u8]> = row.path.chunks(size).collect();
            meshctl_api::MessageRoute {
                hops: hops_of(&known, &hashes, row.flooded),
                flooded: row.flooded,
                snr: row.snr,
                rssi: row.rssi,
                scope: row.scope,
                at: row.at,
            }
        })
        .collect()
}

/// The region a packet went in, by its name, told by the regions whose keys
/// are known: found nearby, of a channel, or the radio's default; `?` when
/// none of them is it.
async fn region_name(
    store: &Store,
    radio: RadioId,
    mesh: &Mesh,
    region: &mesh::RegionCode,
) -> Result<String> {
    let mut names: Vec<String> = store
        .latest_regions(radio)
        .await?
        .into_iter()
        .flat_map(|(_, reply, _)| match reply {
            meshctl_api::RequestReply::Regions { regions, .. } => regions,
            _ => Vec::new(),
        })
        .collect();
    names.extend(store.channel_scopes(radio).await?.into_values());
    names.extend(mesh.read(|state| {
        state
            .meshcore
            .as_ref()
            .and_then(|s| s.default_scope.clone().flatten())
            .map(|(name, _)| name)
    }));
    let found = names.into_iter().find(|name| {
        name != "*"
            && meshctl_core::meshcore::raw::transport_code(
                &crate::meshcore::hashtag_secret(name),
                region.payload_type,
                &region.payload,
            ) == region.code
    });
    Ok(found.unwrap_or_else(|| "?".to_owned()))
}

/// Keeps a copy of a message heard and the way it came, its region by its
/// name. Tells of the message, if kept already.
async fn keep_route(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    mesh: &Mesh,
    route: &mesh::Route,
) -> Result<()> {
    let scope = match &route.region {
        None => None,
        Some(region) => Some(region_name(store, radio, mesh, region).await?),
    };
    let row = crate::store::RouteRow {
        path: route.hops.concat(),
        hash_size: route
            .hops
            .first()
            .map_or(1, |h| i64::try_from(h.len()).unwrap_or(1)),
        flooded: route.flooded,
        snr: Some(route.snr),
        rssi: Some(route.rssi),
        scope,
        at: i64::from(route.at),
    };
    if let Some(message) = store.add_route(radio, route.packet_id, &row).await? {
        let _ = changes.send(Change::MessageHeard {
            radio,
            id: message.id,
            heard: message.heard,
            repeaters: message.repeaters,
            scope: message.scope,
        });
    }
    Ok(())
}

/// Gives up on the requests waited for too long, traceroutes among them.
async fn expire(store: &Store, changes: &broadcast::Sender<Change>, radio: RadioId) -> Result<()> {
    for request in store
        .expire_requests(radio, now() - REQUEST_TIMEOUT)
        .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    crate::waypoints::expire(store, changes, radio).await
}

/// Who wrote a message on a MeshCore radio, as named on a channel or in a
/// room: the radio, for ours; a room's post, its author.
fn author(mesh: &Mesh, message: &Message) -> Option<String> {
    if message.outgoing {
        mesh.read(|s| s.meshcore.as_ref().map(|m| m.name.clone()))
    } else {
        message.sender_name.clone().or_else(|| {
            mesh.read(|s| {
                s.nodes
                    .get(&message.author.unwrap_or(message.from))
                    .and_then(|n| n.user.as_ref())
                    .map(|u| u.long_name.clone())
            })
        })
    }
}

/// Whether a radio has a channel in that slot that it uses.
pub(crate) fn has_channel(mesh: &Mesh, channel: u32) -> bool {
    mesh.read(|state| {
        state
            .channels
            .get(&channel)
            .is_some_and(|c| c.role != "DISABLED")
    })
}

/// Checks what a client asks a radio to be, and says where it is.
pub fn validate(settings: &RadioSettings) -> Result<Transport, ErrorCode> {
    let name = settings.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ErrorCode::InvalidName);
    }
    // Each kind serves on a port of its own, taken when none is given.
    let port = match settings.kind {
        RadioKind::Meshtastic => meshctl_core::link::DEFAULT_TCP_PORT,
        RadioKind::Meshcore => meshctl_core::meshcore::DEFAULT_TCP_PORT,
    };
    let transport =
        Transport::parse(settings.address.trim(), port).map_err(|_| ErrorCode::InvalidAddress)?;
    if !valid_badge(&settings.badge) {
        return Err(ErrorCode::InvalidBadge);
    }
    Ok(transport)
}

/// The most a button's text takes, in bytes.
const TEXT_MAX: usize = 4;

/// The most an emoji takes, in characters: enough for those joined of
/// several, such as families or flags of regions.
const EMOJI_MAX: usize = 16;

/// Whether a radio's button can be shown: a colour of the palette, or one as
/// `#rrggbb`; up to four bytes of text, as a node's short name; or an emoji,
/// which has no letters, digits or spaces.
fn valid_badge(badge: &Badge) -> bool {
    let color = badge.color.as_deref().is_none_or(|color| {
        BADGE_COLORS.contains(&color)
            || (color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit()))
    });
    let shown = |text: &str| text.chars().all(|c| !c.is_whitespace() && !c.is_control());
    let content = match &badge.content {
        BadgeContent::Initials | BadgeContent::Icon { .. } => true,
        BadgeContent::Text { text } => {
            let text = text.trim();
            // As Meshtastic's short names: four bytes, which is four letters,
            // fewer with accents, or one emoji.
            (1..=TEXT_MAX).contains(&text.len()) && shown(text)
        }
        BadgeContent::Emoji { emoji } => {
            let emoji = emoji.trim();
            (1..=EMOJI_MAX).contains(&emoji.chars().count())
                && shown(emoji)
                && !emoji.chars().any(|c| c.is_alphanumeric())
        }
    };
    color && content
}

/// Settings as they are kept: trimmed.
pub fn tidy(settings: RadioSettings) -> RadioSettings {
    RadioSettings {
        name: settings.name.trim().to_owned(),
        address: settings.address.trim().to_owned(),
        badge: Badge {
            color: settings.badge.color.map(|color| color.to_ascii_lowercase()),
            content: match settings.badge.content {
                BadgeContent::Text { text } => BadgeContent::Text {
                    text: text.trim().to_owned(),
                },
                BadgeContent::Emoji { emoji } => BadgeContent::Emoji {
                    emoji: emoji.trim().to_owned(),
                },
                content => content,
            },
        },
        ..settings
    }
}

/// A radio's mesh, and the tasks that keep it: its link when it is enabled,
/// and always one that passes its events on and one that stores them.
struct Running {
    mesh: Mesh,
    protocol: Protocol,
    enabled: bool,
    tasks: Vec<JoinHandle<()>>,
}

/// The driver a kind of radio takes.
fn protocol_of(kind: RadioKind) -> Protocol {
    match kind {
        RadioKind::Meshtastic => Protocol::Meshtastic,
        RadioKind::Meshcore => Protocol::Meshcore,
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // The link closes with its task.
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Radios {
    pub(crate) store: Store,
    running: Arc<Mutex<HashMap<RadioId, Running>>>,
    /// The radios searching for regions, and how many repeaters each has
    /// still to ask.
    searching: Arc<Mutex<HashMap<RadioId, u32>>>,
    /// The nodes each radio logged in to since it last connected, rooms,
    /// repeaters and sensors: what each
    /// let it in as, and when.
    logins: Arc<Mutex<HashMap<(RadioId, u32), LoginHeld>>>,
    /// The logins made before a request, one at a time to each node: what
    /// comes while one is under way waits for it, rather than make another.
    logging_in: Arc<Mutex<HashMap<(RadioId, u32), LoginGate>>>,
    /// How the last of those failed, and when: what waited for it fails
    /// alike, with nothing sent again.
    login_failed: Arc<Mutex<HashMap<(RadioId, u32), LoginFailed>>>,
    /// How busy each radio last said it hears its channel, in percent, and
    /// when.
    busy: Arc<Mutex<HashMap<RadioId, (f32, std::time::Instant)>>>,
    /// What other nodes answered each radio of their settings, asked as
    /// their admin. In memory only, secrets and all, as the radio's own.
    remote: Arc<Mutex<HashMap<(RadioId, u32), meshctl_core::mesh::Settings>>>,
    /// The app it runs within, if any.
    host: Arc<std::sync::OnceLock<Arc<dyn crate::Host>>>,
    /// The conversation each client of the event stream shows, and is seen,
    /// by the number it was given.
    viewing: Arc<Mutex<HashMap<u64, meshctl_api::events::Viewing>>>,
    /// When each radio last sent a waypoint: the firmware refuses another
    /// too soon after.
    pub(crate) waypoint_sent: Arc<Mutex<HashMap<RadioId, std::time::Instant>>>,
    changes: broadcast::Sender<Change>,
    stopped: watch::Sender<bool>,
}

impl Radios {
    /// Starts every radio in the store, and connects to the enabled ones.
    pub async fn start(store: Store) -> Result<Self> {
        let radios = Self {
            store,
            running: Arc::default(),
            searching: Arc::default(),
            logins: Arc::default(),
            logging_in: Arc::default(),
            login_failed: Arc::default(),
            busy: Arc::default(),
            remote: Arc::default(),
            host: Arc::default(),
            viewing: Arc::default(),
            waypoint_sent: Arc::default(),
            changes: broadcast::channel(EVENT_BACKLOG).0,
            stopped: watch::channel(false).0,
        };
        for record in radios.store.radios().await? {
            let state = radios.stored_state(record.id).await?;
            radios.run(&record, state);
        }
        radios.keep_clean();
        crate::auto_replies::spawn(radios.clone());
        crate::notifications::spawn(radios.clone());
        Ok(radios)
    }

    /// Cleans away what is kept for longer than it is kept: on starting, and
    /// once a day, until stopped.
    fn keep_clean(&self) {
        let radios = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(CLEANING_INTERVAL);
            loop {
                tokio::select! {
                    _ = every.tick() => {
                        if let Err(err) = radios.clean().await {
                            tracing::warn!(error = format_args!("{err:#}"), "cannot clean");
                        }
                    }
                    () = radios.stopped() => return,
                }
            }
        });
    }

    /// How long what meshctl keeps is kept.
    pub async fn retention(&self) -> Result<meshctl_api::Retention> {
        self.store.retention().await
    }

    /// Changes how long what meshctl keeps is kept, and cleans away at once
    /// what is now kept for longer.
    pub async fn set_retention(&self, retention: meshctl_api::Retention) -> Result<()> {
        self.store.set_retention(&retention).await?;
        let _ = self.changes.send(Change::Retention(retention));
        self.clean().await?;
        Ok(())
    }

    /// What is kept, and how much room it takes.
    pub async fn storage(&self) -> Result<meshctl_api::Storage> {
        self.store.storage().await
    }

    /// Cleans away what is kept for longer than it is kept: the packets but
    /// each node's latest of each kind, the messages, and the nodes meshctl
    /// alone keeps, not heard for so long. Never a node the radio has, nor a
    /// favourite, nor one it is not known of yet. Every client hears of it
    /// if anything went.
    pub async fn clean(&self) -> Result<meshctl_api::Cleaned> {
        let retention = self.store.retention().await?;
        let now = now();
        let before = |days: u32| now - i64::from(days) * DAY;
        let mut cleaned = meshctl_api::Cleaned::default();
        if let Some(days) = retention.packets {
            cleaned.packets = self.store.prune_packets(before(days)).await?;
        }
        if let Some(days) = retention.messages {
            cleaned.messages = self.store.prune_messages(before(days)).await?;
        }
        if let Some(days) = retention.nodes {
            let before = before(days);
            let meshes: Vec<(RadioId, Mesh)> = self
                .lock()
                .iter()
                .map(|(id, running)| (*id, running.mesh.clone()))
                .collect();
            for (id, mesh) in meshes {
                let old: Vec<u32> = mesh.read(|state| {
                    state
                        .nodes
                        .values()
                        .filter(|n| n.on_radio == Some(false) && !n.favorite)
                        .filter(|n| n.last_heard.is_some_and(|at| i64::from(at) < before))
                        .map(|n| n.num)
                        .collect()
                });
                for num in old {
                    mesh.drop_node(num);
                    self.store.remove_node(id, num).await?;
                    cleaned.nodes += 1;
                }
            }
        }
        self.cleaned(cleaned).await
    }

    /// Forgets the conversations of channels no radio has any more.
    pub async fn forget_gone_channels(&self) -> Result<meshctl_api::Cleaned> {
        let messages = self.store.forget_gone_channels().await?;
        self.cleaned(meshctl_api::Cleaned {
            messages,
            ..Default::default()
        })
        .await
    }

    /// Forgets the direct conversations with nodes meshctl no longer keeps.
    pub async fn forget_gone_directs(&self) -> Result<meshctl_api::Cleaned> {
        let messages = self.store.forget_gone_directs().await?;
        self.cleaned(meshctl_api::Cleaned {
            messages,
            ..Default::default()
        })
        .await
    }

    /// The room freed given back, and every client told, if anything went.
    async fn cleaned(&self, mut cleaned: meshctl_api::Cleaned) -> Result<meshctl_api::Cleaned> {
        cleaned.bytes = self.store.reclaim().await?;
        if cleaned != meshctl_api::Cleaned::default() {
            tracing::info!(
                packets = cleaned.packets,
                nodes = cleaned.nodes,
                messages = cleaned.messages,
                bytes = cleaned.bytes,
                "cleaned"
            );
            let _ = self.changes.send(Change::Cleaned(cleaned));
        }
        Ok(cleaned)
    }

    /// Everything that changes from now on, on every radio.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// What a radio knows, live if it is connected and as last stored if not.
    pub fn mesh(&self, id: RadioId) -> Option<Mesh> {
        self.lock().get(&id).map(|running| running.mesh.clone())
    }

    /// Which mesh a radio speaks, if there is such a radio.
    fn protocol(&self, id: RadioId) -> Option<Protocol> {
        self.lock().get(&id).map(|running| running.protocol)
    }

    /// How a radio's link is doing; [`LinkState::Disabled`] for one that is
    /// not meant to be connected, or not running at all.
    pub fn link(&self, id: RadioId) -> LinkState {
        match self.lock().get(&id) {
            Some(running) if running.enabled => {
                crate::web::convert::link(running.mesh.read(|state| state.link))
            }
            _ => LinkState::Disabled,
        }
    }

    /// The link that shares a node a radio knows of (its own, with none), as
    /// its kind's apps read it, in a QR or as text; none while the node has
    /// not said who it is, or without its key.
    pub fn contact(&self, id: RadioId, kind: RadioKind, node: Option<u32>) -> Option<String> {
        self.mesh(id)?.read(|state| {
            let local = state.local.as_ref().map(|l| l.num);
            let num = node.or(local)?;
            let user = state.nodes.get(&num).and_then(|n| n.user.as_ref());
            match kind {
                RadioKind::Meshtastic => {
                    // The radio's own, as its owner says, with its key as the
                    // node list has it if its owner came without; any other,
                    // as its node info said.
                    let shared = match state.settings.owner.clone() {
                        Some(mut owner) if Some(num) == local => {
                            if owner.public_key.is_empty()
                                && let Some(user) = user
                            {
                                owner.public_key.clone_from(&user.public_key);
                            }
                            owner
                        }
                        _ => meshtastic_user(num, user?),
                    };
                    (!shared.public_key.is_empty() || Some(num) == local)
                        .then(|| meshtastic_contact(num, shared))
                }
                RadioKind::Meshcore => {
                    let user = user?;
                    meshcore_contact(&user.long_name, &user.public_key, &user.role)
                }
            }
        })
    }

    /// A radio's link and what it said of itself, if there is such a radio.
    pub fn status(&self, id: RadioId) -> Option<Status> {
        let (local, since) = self.mesh(id)?.read(|state| {
            (
                state.local.as_ref().map(crate::web::convert::local),
                state.link_since,
            )
        });
        Some(Status {
            link: self.link(id),
            since: since.map(i64::from),
            local,
        })
    }

    /// Forgets a message a radio keeps, and the reactions to it, and tells
    /// every client; nothing of it changes on the mesh. False if there is no
    /// such message.
    pub async fn forget_message(&self, id: RadioId, message: MessageId) -> Result<bool> {
        let forgotten = self.store.forget_message(id, message).await?;
        if forgotten.is_empty() {
            return Ok(false);
        }
        let _ = self.changes.send(Change::MessagesForgotten(id, forgotten));
        Ok(true)
    }

    /// Forgets a conversation's messages, only here, and tells every
    /// client: a channel's, which stays on the radio; or those exchanged
    /// with a node. False if there is no such radio, or channel.
    pub async fn forget_conversation(&self, id: RadioId, peer: Peer) -> Result<bool> {
        if self.mesh(id).is_none() || !self.store.forget_conversation(id, peer).await? {
            return Ok(false);
        }
        tracing::info!(radio = id, ?peer, "conversation forgotten");
        let _ = self.changes.send(Change::ConversationForgotten(id, peer));
        Ok(true)
    }

    /// Sends a message through a radio, and keeps it as sent.
    pub async fn send(&self, id: RadioId, message: NewMessage) -> Result<Message, SendFailure> {
        let mesh = self.mesh(id).ok_or(SendFailure::NoSuchRadio)?;
        // A reaction: to a message, and an emoji, a few characters beyond
        // plain text, none a space.
        if message.emoji {
            let emoji = message.text.trim();
            if message.reply_id.is_none()
                || emoji.is_empty()
                || emoji.len() > 32
                || emoji.is_ascii()
                || emoji.chars().any(char::is_whitespace)
            {
                return Err(SendFailure::Invalid);
            }
        }
        let (to, channel) = match (message.channel, message.node) {
            (Some(channel), None) => {
                let usable = mesh.read(|state| {
                    state
                        .channels
                        .get(&channel)
                        .is_some_and(|c| c.role != "DISABLED")
                });
                if !usable {
                    return Err(SendFailure::Invalid);
                }
                (mesh::BROADCAST, channel)
            }
            // A direct message goes on the primary channel; with the
            // recipient's key, when the firmware has it, it is private anyway.
            (None, Some(node)) if node != mesh::BROADCAST => (node, 0),
            _ => return Err(SendFailure::Invalid),
        };
        // On MeshCore, which has neither, a reply or a reaction goes as its
        // apps send them: what goes on the air, kept as it went; and a reply
        // in the region its message came in, unless another is asked for.
        let meshcore = self.protocol(id) == Some(Protocol::Meshcore);
        // No reactions in a room: MeshCore's rooms have none, and one would
        // go as a post.
        let to_room = meshcore
            && mesh.read(|state| {
                state
                    .nodes
                    .get(&to)
                    .and_then(|n| n.user.as_ref())
                    .is_some_and(|u| u.role == "ROOM")
            });
        if to_room && message.emoji {
            return Err(SendFailure::Invalid);
        }
        let mut wire = message.text.clone();
        let mut region_of_reply = None;
        if meshcore && let Some(answered) = message.reply_id {
            let target = self
                .store
                .message_by_packet(id, answered)
                .await?
                .ok_or(SendFailure::Invalid)?;
            let sender = author(&mesh, &target);
            let on_channel = to == mesh::BROADCAST;
            // A reply in a group, a channel or a room, names whom it answers.
            let group = on_channel || to_room;
            if message.emoji {
                let at = target
                    .sent_at
                    .and_then(|at| u32::try_from(at).ok())
                    .ok_or(SendFailure::Invalid)?;
                let named = if on_channel {
                    Some(sender.as_deref().ok_or(SendFailure::Invalid)?)
                } else {
                    None
                };
                wire = crate::reactions::build(message.text.trim(), named, &target.text, at)
                    .ok_or(SendFailure::Invalid)?;
            } else if group && let Some(sender) = &sender {
                wire = format!("@[{sender}]: {}", message.text);
            }
            region_of_reply = target.scope.filter(|s| s != "?");
        }
        let message = NewMessage {
            scope: message.scope.clone().or(region_of_reply),
            ..message
        };
        // The region it floods in: as asked for it alone, its channel's, or
        // the radio's default; and the one kept with it, if any.
        let (scope, scope_kept) = if meshcore {
            let asked = match &message.scope {
                Some(name) => Some(crate::meshcore::scope_name(name).ok_or(SendFailure::Invalid)?),
                None if to == mesh::BROADCAST => {
                    let print = mesh.read(|state| {
                        crate::channels::slots_of(state)
                            .into_iter()
                            .find(|s| s.index == channel)
                            .map(|s| s.fingerprint)
                    });
                    match print {
                        Some(print) => self.store.channel_scopes(id).await?.remove(&print),
                        None => None,
                    }
                }
                None => None,
            };
            let default = mesh.read(|state| {
                state
                    .meshcore
                    .as_ref()
                    .and_then(|s| s.default_scope.clone().flatten())
                    .map(|(name, _)| name)
            });
            match asked.as_deref() {
                Some("*") => (Some(mesh::Scope::Unscoped), None),
                Some(name) => (
                    Some(mesh::Scope::Region(crate::meshcore::hashtag_secret(name))),
                    Some(name.to_owned()),
                ),
                None => (None, default),
            }
        } else if message.scope.is_some() {
            return Err(SendFailure::Invalid);
        } else {
            (None, None)
        };
        let outgoing = OutgoingText {
            to,
            channel,
            text: wire.clone(),
            reply_id: message.reply_id.filter(|_| !meshcore),
            emoji: message.emoji && !meshcore,
            scope,
            sent_at: None,
        };
        if self.link(id) != LinkState::Ready {
            return Err(SendFailure::NotConnected);
        }
        let sent = mesh
            .send_text(&outgoing)
            .await
            .map_err(|error| match error {
                SendError::NotConnected => SendFailure::NotConnected,
                SendError::BadText | SendError::Refused | SendError::Unsupported => {
                    SendFailure::Invalid
                }
            })?;
        let from = mesh.read(|state| state.local.as_ref().map_or(0, |l| l.num));
        let row = NewMessageRow {
            packet_id: sent.packet_id,
            from,
            sender_name: None,
            to,
            channel,
            // As it went on the air, a MeshCore reply's `@[Sender]: ` and all,
            // as what reacts to it hashes; a reaction, its emoji.
            text: if message.emoji { message.text } else { wire },
            at: now(),
            outgoing: true,
            // What cannot be acknowledged is as sent as it will ever be.
            status: if sent.acknowledged {
                MessageStatus::Sending
            } else {
                MessageStatus::Sent
            },
            reply_id: message.reply_id,
            emoji: message.emoji,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            scope: scope_kept,
            sent_at: sent.sent_at.map(i64::from),
            author: None,
            position: None,
        };
        let kept = self
            .store
            .add_message(id, &row)
            .await?
            .ok_or_else(|| anyhow::anyhow!("a packet id was used twice"))?;
        let _ = self.changes.send(Change::Message(id, kept.clone()));
        Ok(kept)
    }

    /// Sends where the radio is to a channel or a node, as a position, and
    /// keeps it among the conversation's messages as it went: rounded as
    /// the channel shares positions, which a direct message's is the
    /// primary's. Only on Meshtastic, which has positions to send.
    pub async fn share_position(
        &self,
        id: RadioId,
        share: PositionShare,
    ) -> Result<Message, SendFailure> {
        let mesh = self.mesh(id).ok_or(SendFailure::NoSuchRadio)?;
        if self.protocol(id) == Some(Protocol::Meshcore) {
            return Err(SendFailure::Invalid);
        }
        let (to, channel) = match (share.channel, share.node) {
            (Some(channel), None) => (mesh::BROADCAST, channel),
            (None, Some(node)) if node != mesh::BROADCAST => (node, 0),
            _ => return Err(SendFailure::Invalid),
        };
        if self.link(id) != LinkState::Ready {
            return Err(SendFailure::NotConnected);
        }
        let (here, precision) = mesh.read(|state| {
            let here = state
                .local
                .as_ref()
                .and_then(|l| state.nodes.get(&l.num)?.position);
            (here, shared_precision(&state.settings, channel))
        });
        let precision = precision.ok_or(SendFailure::Invalid)?;
        let here = here.ok_or(SendFailure::NotShared)?;
        if precision == 0 {
            return Err(SendFailure::NotShared);
        }
        // In ten millionths of a degree, as the protobufs carry it.
        #[allow(clippy::cast_possible_truncation)]
        let degrees = |d: f64| (d * 1e7).round() as i32;
        let (latitude, longitude) = (degrees(here.latitude), degrees(here.longitude));
        let position = meshctl_proto::meshtastic::Position {
            latitude_i: Some(latitude),
            longitude_i: Some(longitude),
            altitude: here.altitude,
            time: u32::try_from(now()).unwrap_or(0),
            ..Default::default()
        };
        let packet_id = mesh
            .send_position(to, channel, position)
            .map_err(|error| match error {
                SendError::NotConnected => SendFailure::NotConnected,
                SendError::BadText | SendError::Refused | SendError::Unsupported => {
                    SendFailure::Invalid
                }
            })?;
        let from = mesh.read(|state| state.local.as_ref().map_or(0, |l| l.num));
        // As the others have it: rounded as the firmware rounds it on its way.
        let went = |coordinate| f64::from(mesh::truncated(coordinate, precision)) * 1e-7;
        let row = NewMessageRow {
            packet_id,
            from,
            sender_name: None,
            to,
            channel,
            text: String::new(),
            at: now(),
            outgoing: true,
            status: MessageStatus::Sending,
            reply_id: None,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            scope: None,
            sent_at: None,
            author: None,
            position: Some(meshctl_api::Position {
                latitude: went(latitude),
                longitude: went(longitude),
                altitude: here.altitude,
                radius: mesh::radius(precision),
                source: None,
            }),
        };
        let kept = self
            .store
            .add_message(id, &row)
            .await?
            .ok_or_else(|| anyhow::anyhow!("a packet id was used twice"))?;
        let _ = self.changes.send(Change::Message(id, kept.clone()));
        Ok(kept)
    }

    /// A MeshCore node the radio knows that takes logins: its mesh, or why not.
    fn login_mesh(&self, id: RadioId, node: u32) -> Result<Mesh, LoginFailure> {
        let mesh = self.mesh(id).ok_or(LoginFailure::NoSuchRadio)?;
        // Repeaters, rooms and sensors take logins; a companion does not.
        let takes = self.protocol(id) == Some(Protocol::Meshcore)
            && mesh.read(|state| {
                state
                    .nodes
                    .get(&node)
                    .and_then(|n| n.user.as_ref())
                    .is_some_and(|u| matches!(u.role.as_str(), "REPEATER" | "ROOM" | "SENSOR"))
            });
        if takes {
            Ok(mesh)
        } else {
            Err(LoginFailure::NoLogin)
        }
    }

    /// The radio's login to a node: what it was let in as since it last
    /// connected, and whether a password is kept for it.
    pub async fn node_login(&self, id: RadioId, node: u32) -> Result<NodeLogin, LoginFailure> {
        self.login_mesh(id, node)?;
        let now = self
            .logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(id, node))
            .copied();
        Ok(NodeLogin {
            access: now.map(|(access, _)| access),
            at: now.map(|(_, at)| at),
            remembered: self.store.node_password(id, node).await?.is_some(),
        })
    }

    /// Logs the radio in to a node: with the password given, kept once the
    /// node takes it; with none, with the one kept, or as a guest. A room
    /// then sends it the posts it does not have.
    pub async fn log_in_node(
        &self,
        id: RadioId,
        node: u32,
        password: Option<String>,
    ) -> Result<NodeLogin, LoginFailure> {
        let mesh = self.login_mesh(id, node)?;
        // One the radio no longer has: said so, with nothing sent.
        if mesh.read(|state| state.nodes.get(&node).and_then(|n| n.on_radio)) == Some(false) {
            return Err(LoginFailure::NotOnRadio);
        }
        if self.link(id) != LinkState::Ready {
            return Err(LoginFailure::NotConnected);
        }
        let given = password.is_some();
        let password = match password {
            Some(password) => password,
            None => self
                .store
                .node_password(id, node)
                .await?
                .unwrap_or_default(),
        };
        let failure = |error| match error {
            AdminError::NotConnected => LoginFailure::NotConnected,
            AdminError::NoAnswer => LoginFailure::NoAnswer,
            AdminError::UnknownNode | AdminError::OwnNode => LoginFailure::NoLogin,
            AdminError::Refused(why) if why == "WRONG_PASSWORD" => LoginFailure::WrongPassword,
            AdminError::Refused(why) if why == "NOT_FOUND" => LoginFailure::NotOnRadio,
            AdminError::Refused(why) => LoginFailure::Refused(why),
            AdminError::Unsupported | AdminError::NoPath => {
                LoginFailure::Refused("UNSUPPORTED".to_owned())
            }
        };
        let sent = mesh
            .meshcore_login(node, &password)
            .await
            .map_err(failure)?;
        // How long it is waited for, for whoever watches it.
        let _ = self.changes.send(Change::LoginWaiting {
            radio: id,
            node,
            wait_ms: u32::try_from(sent.wait.as_millis()).unwrap_or(u32::MAX),
        });
        let access = sent.answer().await.map_err(failure)?;
        let access = match access {
            meshctl_core::meshcore::protocol::Access::ReadOnly => NodeAccess::ReadOnly,
            meshctl_core::meshcore::protocol::Access::ReadWrite => NodeAccess::ReadWrite,
            meshctl_core::meshcore::protocol::Access::Admin => NodeAccess::Admin,
        };
        // A password given is kept once it is known to be right: a room lets
        // a wrong one in to read, so there only one that lets in to more; a
        // repeater or a sensor says nothing to a wrong one.
        let room = mesh.read(|state| {
            state
                .nodes
                .get(&node)
                .and_then(|n| n.user.as_ref())
                .is_some_and(|u| u.role == "ROOM")
        });
        if given && (access != NodeAccess::ReadOnly || !room) {
            self.store
                .set_node_password(id, node, Some(&password))
                .await?;
        }
        let at = now();
        self.logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((id, node), (access, at));
        self.node_login(id, node).await
    }

    /// Logs in to a node before a request, with the password kept or as a
    /// guest: one at a time to each node. One that came while another was
    /// under way takes how that ended, logged in or failed, rather than send
    /// another login; one that comes later tries again.
    async fn log_in_once(
        &self,
        id: RadioId,
        node: u32,
        arrived: std::time::Instant,
    ) -> Result<(), LoginFailure> {
        let gate = Arc::clone(
            self.logging_in
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry((id, node))
                .or_default(),
        );
        let _turn = gate.lock().await;
        let held = self
            .logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&(id, node));
        if held {
            return Ok(());
        }
        let failed = self
            .login_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(id, node))
            .filter(|(at, _)| *at > arrived)
            .map(|(_, failure)| failure.again());
        if let Some(failure) = failed {
            return Err(failure);
        }
        match self.log_in_node(id, node, None).await {
            Ok(_) => Ok(()),
            Err(failure) => {
                self.login_failed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert((id, node), (std::time::Instant::now(), failure.again()));
                Err(failure)
            }
        }
    }

    /// Sends a command to the console of a node, as its admin: logged in to
    /// first with the password kept, if the radio has not since it
    /// connected. Its answer comes as a console event; nothing says it
    /// arrived.
    pub async fn node_command(
        &self,
        id: RadioId,
        node: u32,
        command: &str,
    ) -> Result<(), LoginFailure> {
        let command = command.trim();
        if command.is_empty() || command.len() > meshctl_core::radio::MAX_TEXT_BYTES {
            return Err(LoginFailure::Invalid);
        }
        // The radio's own: its console, at once, with no login and nothing
        // sent.
        let mesh = self.mesh(id).ok_or(LoginFailure::NoSuchRadio)?;
        let own = mesh.read(|state| state.local.as_ref().map(|l| l.num)) == Some(node);
        if own && self.protocol(id) == Some(Protocol::Meshcore) {
            if self.link(id) != LinkState::Ready {
                return Err(LoginFailure::NotConnected);
            }
            return mesh
                .meshcore_run(command)
                .await
                .map(|_| ())
                .map_err(|error| match error {
                    AdminError::NoAnswer | AdminError::NotConnected => LoginFailure::NotConnected,
                    AdminError::Unsupported => LoginFailure::Refused("UNSUPPORTED".to_owned()),
                    AdminError::Refused(why) => LoginFailure::Refused(why),
                    _ => LoginFailure::NoLogin,
                });
        }
        let mesh = self.login_mesh(id, node)?;
        if self.link(id) != LinkState::Ready {
            return Err(LoginFailure::NotConnected);
        }
        let held = |radios: &Self| {
            radios
                .logins
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(id, node))
                .map(|(access, _)| *access)
        };
        if held(self).is_none() {
            self.log_in_once(id, node, std::time::Instant::now())
                .await?;
        }
        if held(self) != Some(NodeAccess::Admin) {
            return Err(LoginFailure::NotAdmin);
        }
        mesh.meshcore_command(node, command)
            .await
            .map_err(|error| match error {
                AdminError::NoAnswer | AdminError::NotConnected => LoginFailure::NotConnected,
                AdminError::UnknownNode | AdminError::OwnNode => LoginFailure::NoLogin,
                AdminError::Refused(why) if why == "NOT_FOUND" => LoginFailure::NotOnRadio,
                AdminError::Refused(why) => LoginFailure::Refused(why),
                AdminError::Unsupported | AdminError::NoPath => {
                    LoginFailure::Refused("UNSUPPORTED".to_owned())
                }
            })
    }

    /// Forgets the password kept for a node; the login, if any, stays.
    pub async fn forget_node_password(
        &self,
        id: RadioId,
        node: u32,
    ) -> Result<NodeLogin, LoginFailure> {
        self.login_mesh(id, node)?;
        self.store.set_node_password(id, node, None).await?;
        self.node_login(id, node).await
    }

    /// Sends again one of ours on a MeshCore channel that no repeater was
    /// heard passing on: the same packet, by the clock it first went by, in
    /// the region it went in, so that what is heard of it now is told as it.
    /// Kept as it was; none if there is no such message.
    pub async fn resend(
        &self,
        id: RadioId,
        message: MessageId,
    ) -> Result<Option<Message>, SendFailure> {
        let mesh = self.mesh(id).ok_or(SendFailure::NoSuchRadio)?;
        let Some(kept) = self.store.radio_message(id, message).await? else {
            return Ok(None);
        };
        let at = kept.sent_at.and_then(|at| u32::try_from(at).ok());
        let (true, true, Some(at)) = (
            self.protocol(id) == Some(Protocol::Meshcore),
            kept.outgoing && kept.to == mesh::BROADCAST,
            at,
        ) else {
            return Err(SendFailure::Invalid);
        };
        // Its region by name, or none if it went unscoped.
        let scope = match kept.scope.as_deref().filter(|s| *s != "?") {
            Some(name) => mesh::Scope::Region(crate::meshcore::hashtag_secret(name)),
            None => mesh::Scope::Unscoped,
        };
        // A reaction kept as its emoji went as MeshCore One writes it.
        let text = if kept.emoji {
            let answered = kept.reply_id.ok_or(SendFailure::Invalid)?;
            let target = self
                .store
                .message_by_packet(id, answered)
                .await?
                .ok_or(SendFailure::Invalid)?;
            let target_at = target
                .sent_at
                .and_then(|at| u32::try_from(at).ok())
                .ok_or(SendFailure::Invalid)?;
            let sender = author(&mesh, &target).ok_or(SendFailure::Invalid)?;
            crate::reactions::build(&kept.text, Some(&sender), &target.text, target_at)
                .ok_or(SendFailure::Invalid)?
        } else {
            kept.text.clone()
        };
        let outgoing = OutgoingText {
            to: mesh::BROADCAST,
            channel: kept.channel,
            text,
            reply_id: None,
            emoji: false,
            scope: Some(scope),
            sent_at: Some(at),
        };
        if self.link(id) != LinkState::Ready {
            return Err(SendFailure::NotConnected);
        }
        mesh.send_text(&outgoing)
            .await
            .map_err(|error| match error {
                SendError::NotConnected => SendFailure::NotConnected,
                SendError::BadText | SendError::Refused | SendError::Unsupported => {
                    SendFailure::Invalid
                }
            })?;
        Ok(Some(kept))
    }

    /// The ways a message came, each copy heard: each hop with the nodes its
    /// hash may be. None if there is no such radio or message.
    pub async fn routes(
        &self,
        id: RadioId,
        message: MessageId,
    ) -> Result<Option<Vec<meshctl_api::MessageRoute>>> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        let rows = self.store.routes(id, message).await?;
        Ok(rows.map(|rows| routes_of(&mesh, rows)))
    }

    /// The ways a packet a MeshCore radio heard came, each copy: each hop
    /// with the nodes its hash may be. None if there is no such radio or
    /// packet.
    pub async fn packet_routes(
        &self,
        id: RadioId,
        packet: SightingId,
    ) -> Result<Option<Vec<meshctl_api::MessageRoute>>> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        let rows = self.store.meshcore_routes(id, packet).await?;
        Ok(rows.map(|rows| routes_of(&mesh, rows)))
    }

    /// Every conversation a radio has: its channels, whether or not anything
    /// was said in them, and every node it exchanged messages with, the most
    /// recent first.
    pub async fn conversations(&self, id: RadioId) -> Result<Option<Vec<Conversation>>> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        let kept = self.store.conversations(id).await?;
        let channels: Vec<u32> = mesh.read(|state| {
            state
                .channels
                .values()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect()
        });
        let mut conversations: Vec<Conversation> = channels
            .into_iter()
            .map(|channel| {
                let peer = Peer::Channel { channel };
                let last = kept
                    .iter()
                    .find(|c| c.peer == peer)
                    .and_then(|c| c.last.clone());
                Conversation {
                    peer,
                    last,
                    unread: 0,
                    read_up_to: None,
                    muted: false,
                }
            })
            .collect();
        // Channels no longer on the radio keep what was said in them.
        let others: Vec<Conversation> = kept
            .into_iter()
            .filter(|c| !conversations.iter().any(|known| known.peer == c.peer))
            .collect();
        conversations.extend(others);
        // How far each was read, and what was not yet.
        for conversation in &mut conversations {
            let (read, unread) = self.store.read_state(id, conversation.peer).await?;
            conversation.read_up_to = read;
            conversation.unread = unread;
            conversation.muted = self.store.muted(id, conversation.peer).await?;
        }
        Ok(Some(conversations))
    }

    /// Whether a conversation of a radio is silenced; not, if it cannot be
    /// told.
    pub async fn muted(&self, id: RadioId, peer: Peer) -> bool {
        self.store.muted(id, peer).await.unwrap_or(false)
    }

    /// Silences a conversation of a radio, or no longer, and tells every
    /// client: false if there is no such radio, or channel on it.
    pub async fn set_muted(&self, id: RadioId, peer: Peer, muted: bool) -> Result<bool> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(false);
        };
        if !self.store.set_muted(id, peer, muted).await? {
            return Ok(false);
        }
        let _ = self.changes.send(Change::ConversationMuted {
            radio: id,
            peer,
            muted,
        });
        // A Meshtastic channel's, on the radio too, if it is there to take
        // it; if not, once it syncs.
        if matches!(peer, Peer::Channel { .. }) && self.link(id) == LinkState::Ready {
            let store = self.store.clone();
            tokio::spawn(async move { push_mutes(&store, id, &mesh).await });
        }
        Ok(true)
    }

    /// Reads a conversation of a radio up to a message, never back, and
    /// tells every client: false if there is no such conversation.
    pub async fn mark_read(&self, id: RadioId, peer: Peer, up_to: MessageId) -> Result<bool> {
        if self.mesh(id).is_none() {
            return Ok(false);
        }
        let Some(up_to) = self.store.mark_read(id, peer, up_to).await? else {
            return Ok(false);
        };
        let _ = self.changes.send(Change::ConversationRead {
            radio: id,
            peer,
            up_to,
        });
        Ok(true)
    }

    /// The messages exchanged with a peer through a radio, if there is such a
    /// radio.
    pub async fn messages(
        &self,
        id: RadioId,
        peer: Peer,
        before: Option<MessageId>,
        limit: u32,
    ) -> Result<Option<Vec<Message>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.messages(id, peer, before, limit).await?))
    }

    /// Asks a node for something it tells on request; the answer, or the lack
    /// of one, comes on the event stream as the request changes.
    pub async fn request(&self, id: RadioId, new: NewRequest) -> Result<Request, AskFailure> {
        // When it came, for a login under way as it did, which it waits for.
        let arrived = std::time::Instant::now();
        let mesh = self.mesh(id).ok_or(AskFailure::NoSuchRadio)?;
        let origin = mesh.read(|state| state.local.as_ref().map(|l| l.num));
        if new.node == mesh::BROADCAST || origin == Some(new.node) {
            return Err(AskFailure::InvalidNode);
        }
        // MeshCore radios ask only what MeshCore nodes answer, and the other
        // way round; neighbours, both.
        use meshctl_core::meshcore::{anonymous, request};
        let meshcore = self.protocol(id) == Some(Protocol::Meshcore);
        let meshcore_kind = matches!(
            new.kind,
            RequestKind::Regions
                | RequestKind::Status
                | RequestKind::Telemetry
                | RequestKind::Owner
                | RequestKind::AccessList
                | RequestKind::Ping
                | RequestKind::Route
                | RequestKind::Series
        );
        // Neighbours and traceroutes, both.
        let both = matches!(new.kind, RequestKind::Neighbors | RequestKind::Traceroute);
        if meshcore_kind && !meshcore || meshcore && !meshcore_kind && !both {
            return Err(AskFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(AskFailure::NotConnected);
        }
        if !meshcore && !has_channel(&mesh, new.channel) {
            return Err(AskFailure::InvalidChannel);
        }
        if self.store.is_asking(id, new.node, new.kind).await? {
            return Err(AskFailure::Pending);
        }
        if meshcore {
            // What needs a login: as a guest at least, first, unless the
            // radio logged in since it connected.
            let logged = match new.kind {
                RequestKind::Status => Some(request::STATUS),
                RequestKind::Telemetry => Some(request::TELEMETRY),
                RequestKind::Series => Some(request::SERIES),
                RequestKind::Neighbors => Some(request::NEIGHBOURS),
                RequestKind::Owner => Some(request::OWNER),
                RequestKind::AccessList => Some(request::ACCESS_LIST),
                _ => None,
            };
            let held = self
                .logins
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&(id, new.node));
            // Its owner with no login, where its path is known; logged in
            // already, as a login tells its firmware too.
            let mut anonymous_owner = None;
            if new.kind == RequestKind::Owner && !held {
                match mesh.meshcore_ask(new.node, anonymous::OWNER).await {
                    Err(AdminError::NoPath) => {}
                    asked => anonymous_owner = Some(asked),
                }
            }
            // A repeater, a room or a sensor reads a request only from a
            // node logged in to it: a route asked of one, too.
            let serves = mesh.read(|state| {
                state
                    .nodes
                    .get(&new.node)
                    .and_then(|n| n.user.as_ref())
                    .is_some_and(|u| u.role != "CHAT")
            });
            let login = logged.is_some() || new.kind == RequestKind::Route && serves;
            if login && !held && anonymous_owner.is_none() {
                self.log_in_once(id, new.node, arrived)
                    .await
                    .map_err(AskFailure::Login)?;
            }
            let asked = match (anonymous_owner, new.kind, logged) {
                (Some(asked), _, _) => asked,
                (None, RequestKind::Ping, _) => mesh.meshcore_ping(new.node).await,
                (None, RequestKind::Route, _) => mesh.meshcore_discover(new.node).await,
                // Along the path it keeps, there and back, at no pace.
                (None, RequestKind::Traceroute, _) => mesh.meshcore_traceroute(new.node).await,
                (None, RequestKind::Series, _) => {
                    mesh.meshcore_series(new.node, new.seconds.unwrap_or(SERIES_SECONDS))
                        .await
                }
                (None, _, Some(what)) => mesh.meshcore_request(new.node, what).await,
                (None, _, None) => mesh.meshcore_ask(new.node, anonymous::REGIONS).await,
            };
            let tag = asked.map_err(|error| match error {
                AdminError::NoPath => AskFailure::NoPath,
                AdminError::UnknownNode | AdminError::OwnNode => AskFailure::InvalidNode,
                AdminError::Unsupported => AskFailure::Unsupported,
                AdminError::NotConnected | AdminError::NoAnswer | AdminError::Refused(_) => {
                    AskFailure::NotConnected
                }
            })?;
            let request = self
                .store
                .add_request(id, new.node, new.kind, 0, tag, now())
                .await?;
            let _ = self.changes.send(Change::Request(id, request.clone()));
            return Ok(request);
        }
        if let Some(pace) = new.kind.pace() {
            let wait = self.wait(id, pace, now()).await?;
            if wait > 0 {
                return Err(AskFailure::TooSoon { wait });
            }
        }
        let ask = match new.kind {
            // A route traced: a packet of its own, not a request.
            RequestKind::Traceroute => None,
            RequestKind::NodeInfo => Some(Ask::NodeInfo),
            RequestKind::Position => Some(Ask::Position),
            RequestKind::DeviceTelemetry => Some(Ask::DeviceTelemetry),
            RequestKind::EnvironmentTelemetry => Some(Ask::EnvironmentTelemetry),
            RequestKind::PowerTelemetry => Some(Ask::PowerTelemetry),
            RequestKind::LocalStats => Some(Ask::LocalStats),
            RequestKind::Neighbors => Some(Ask::Neighbors),
            RequestKind::Regions
            | RequestKind::Status
            | RequestKind::Telemetry
            | RequestKind::Owner
            | RequestKind::AccessList
            | RequestKind::Ping
            | RequestKind::Route
            | RequestKind::Series => return Err(AskFailure::Unsupported),
        };
        let sent = match ask {
            Some(ask) => mesh.send_request(new.node, new.channel, ask),
            None => mesh.send_traceroute(new.node, new.channel),
        };
        let packet_id = sent.map_err(|error| match error {
            SendError::Unsupported => AskFailure::Unsupported,
            SendError::NotConnected | SendError::BadText | SendError::Refused => {
                AskFailure::NotConnected
            }
        })?;
        let request = self
            .store
            .add_request(id, new.node, new.kind, new.channel, packet_id, now())
            .await?;
        let _ = self.changes.send(Change::Request(id, request.clone()));
        Ok(request)
    }

    /// How long, in seconds, until a radio sends the like of what goes at a
    /// pace again: 0 when it can now.
    async fn wait(&self, id: RadioId, pace: Pace, at: i64) -> Result<i64> {
        Ok(self.store.last_paced_at(id, pace).await?.map_or(0, |last| {
            (pace.interval() - (at - last)).clamp(0, pace.interval())
        }))
    }

    /// A node's telemetry over the latest so many seconds, and the signal it
    /// was heard directly at: `None` if there is no such radio, or it knows
    /// of no such node.
    pub async fn telemetry(
        &self,
        id: RadioId,
        num: u32,
        window: i64,
    ) -> Result<Option<NodeTelemetry>> {
        use meshctl_proto::meshtastic::PortNum;
        use serde_json::Value;

        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        if !mesh.read(|state| state.nodes.contains_key(&num)) {
            return Ok(None);
        }
        let end = now();
        let since = end - window;
        let mut series: BTreeMap<String, Vec<TelemetryPoint>> = BTreeMap::new();
        let mut push = |metric: String, at: i64, value: f64| {
            series
                .entry(metric)
                .or_default()
                .push(TelemetryPoint { at, value });
        };
        for (at, payload) in self.store.telemetry_since(id, num, since).await? {
            let Some(Value::Object(telemetry)) =
                crate::packets::read(PortNum::TelemetryApp as i32, &payload)
            else {
                continue;
            };
            for (kind, prefix) in [
                ("device_metrics", "device"),
                ("environment_metrics", "environment"),
                ("power_metrics", "power"),
            ] {
                let Some(Value::Object(fields)) = telemetry.get(kind) else {
                    continue;
                };
                for (field, value) in fields {
                    let Some(value) = value.as_f64() else {
                        continue;
                    };
                    // How long it has been on only grows; a level over 100
                    // is the firmware's way of saying it is plugged in.
                    let skipped = prefix == "device"
                        && (field == "uptime_seconds"
                            || (field == "battery_level" && value > 100.0));
                    if !skipped {
                        push(format!("{prefix}.{field}"), at, value);
                    }
                }
            }
        }
        for (at, snr, rssi) in self.store.signal_since(id, num, since).await? {
            push("signal.snr".into(), at, snr);
            if let Some(rssi) = rssi {
                #[allow(clippy::cast_precision_loss)]
                push("signal.rssi".into(), at, rssi as f64);
            }
        }
        // How far each packet came from, when it ever came through others:
        // a node always heard directly has nothing to chart.
        let hops = self.store.hops_since(id, num, since).await?;
        if hops.iter().any(|&(_, hops)| hops > 0) {
            for (at, hops) in hops {
                #[allow(clippy::cast_precision_loss)]
                push("signal.hops".into(), at, hops as f64);
            }
        }
        Ok(Some(NodeTelemetry {
            oldest: self.store.oldest_telemetry(id, num).await?,
            series: series
                .into_iter()
                .map(|(metric, points)| TelemetrySeries {
                    metric,
                    points: thin(points, since, end, CHART_POINTS),
                })
                .collect(),
        }))
    }

    /// Marks a node a favourite of a radio, or no longer; `None` if there is
    /// no such radio. Every client hears of it as the node changes.
    pub async fn set_favorite(
        &self,
        id: RadioId,
        num: u32,
        favorite: bool,
    ) -> Result<Option<()>, AdminError> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        mesh.set_favorite(num, favorite).await?;
        Ok(Some(()))
    }

    /// Has a radio forget a node, and with `everything`, forgets all kept of
    /// it besides: `None` if there is no such radio. Every client hears of it
    /// as `node_removed`, and of the rest as `node_forgotten`.
    pub async fn remove_node(
        &self,
        id: RadioId,
        num: u32,
        everything: bool,
    ) -> Result<Result<Option<()>, AdminError>> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(Ok(None));
        };
        if let Err(err) = mesh.remove_node(num).await {
            return Ok(Err(err));
        }
        // Out of the radio only, it stays here, as the radio no longer
        // having it. All of it forgotten, out of the list too: now, rather
        // than once the event is taken, so that what comes from it next
        // finds it gone.
        if everything {
            mesh.drop_node(num);
            self.store.remove_node(id, num).await?;
            self.store.forget_node(id, num).await?;
            let _ = self.changes.send(Change::NodeForgotten(id, num));
        }
        Ok(Ok(Some(())))
    }

    /// What a node a radio knows of last told of each kind: `None` if there
    /// is no such radio, or it knows of no such node.
    pub async fn latest(&self, id: RadioId, num: u32) -> Result<Option<NodeLatest>> {
        use meshctl_proto::meshtastic::{PortNum, Telemetry, telemetry};
        use prost::Message;

        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        if !mesh.read(|state| state.nodes.contains_key(&num)) {
            return Ok(None);
        }
        let last = |portnum: PortNum| async move {
            let sightings = self
                .store
                .latest_payloads(id, num, portnum as i32, 1)
                .await?;
            Ok::<_, anyhow::Error>(sightings.first().map(|(sighting, _)| *sighting))
        };
        let mut sightings = [
            last(PortNum::NodeinfoApp).await?,
            last(PortNum::PositionApp).await?,
            None,
            None,
            None,
            None,
            last(PortNum::NeighborinfoApp).await?,
        ];
        // Telemetry of every kind comes on one port: the latest of each.
        for (sighting, payload) in self
            .store
            .latest_payloads(id, num, PortNum::TelemetryApp as i32, TELEMETRY_LOOKED_INTO)
            .await?
        {
            let slot = match Telemetry::decode(payload.as_slice())
                .ok()
                .and_then(|t| t.variant)
            {
                Some(telemetry::Variant::DeviceMetrics(_)) => 2,
                Some(telemetry::Variant::EnvironmentMetrics(_)) => 3,
                Some(telemetry::Variant::PowerMetrics(_)) => 4,
                Some(telemetry::Variant::LocalStats(_)) => 5,
                _ => continue,
            };
            sightings[slot].get_or_insert(sighting);
            if sightings[2..6].iter().all(Option::is_some) {
                break;
            }
        }
        let mut packets = Vec::with_capacity(sightings.len());
        for sighting in sightings {
            packets.push(match sighting {
                Some(sighting) => self
                    .store
                    .sighting(sighting)
                    .await?
                    .map(crate::packets::packet),
                None => None,
            });
        }
        let mut packets = packets.into_iter();
        let mut next = || packets.next().flatten();
        Ok(Some(NodeLatest {
            node_info: next(),
            position: next(),
            device: next(),
            environment: next(),
            power: next(),
            local_stats: next(),
            neighbors: next(),
        }))
    }

    /// What is amiss with a node a radio knows of, found in what it sent:
    /// `None` if there is no such radio, or it knows of no such node.
    pub async fn checks(&self, id: RadioId, num: u32) -> Result<Option<Vec<NodeCheck>>> {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use meshctl_proto::meshtastic::{HardwareModel, PortNum, User};
        use prost::Message;

        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        let since = u32::try_from(now() - SHARED_KEY_WINDOW).unwrap_or_default();
        let Some(mut sharing) = mesh.read(|state| {
            // A node whose node info was not heard yet has no key to share.
            let key = state
                .nodes
                .get(&num)?
                .user
                .as_ref()
                .map(|u| u.public_key.clone())
                .unwrap_or_default();
            Some(
                state
                    .nodes
                    .values()
                    .filter(|other| {
                        other.num != num
                            && !key.is_empty()
                            && other.last_heard.is_some_and(|heard| heard >= since)
                            && other.user.as_ref().is_some_and(|u| u.public_key == key)
                    })
                    .map(|other| (other.last_heard, other.num))
                    .collect::<Vec<_>>(),
            )
        }) else {
            return Ok(None);
        };
        let mut checks = Vec::new();
        if !sharing.is_empty() {
            sharing.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            checks.push(NodeCheck::PublicKeyShared {
                nodes: sharing.into_iter().map(|(_, num)| num).collect(),
            });
        }

        let users: Vec<User> = self
            .store
            .latest_payloads(id, num, PortNum::NodeinfoApp as i32, NODE_INFOS_CHECKED)
            .await?
            .iter()
            .filter_map(|(_, payload)| User::decode(payload.as_slice()).ok())
            .collect();
        let keys = crate::keys::tell_apart(users.iter().map(|user| user.public_key.as_slice()));
        if keys.len() > 1 {
            checks.push(NodeCheck::PublicKeyConflict {
                keys: keys.into_iter().map(|key| BASE64.encode(key)).collect(),
            });
        }
        let mut models: Vec<String> = Vec::new();
        for user in &users {
            if user.hw_model == HardwareModel::Unset as i32 {
                continue;
            }
            let model = HardwareModel::try_from(user.hw_model).map_or_else(
                |_| format!("UNKNOWN_{}", user.hw_model),
                |model| model.as_str_name().to_owned(),
            );
            if !models.contains(&model) {
                models.push(model);
            }
        }
        if models.len() > 1 {
            checks.push(NodeCheck::HardwareChanged { models });
        }
        Ok(Some(checks))
    }

    /// Forgets what a node was asked for through a radio, but what waits for
    /// its answer: how many were, or `None` if there is no such radio.
    pub async fn forget_requests(
        &self,
        id: RadioId,
        node: u32,
        kind: Option<RequestKind>,
    ) -> Result<Option<u64>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.forget_requests(id, node, kind).await?))
    }

    /// How long until a radio sends each paced kind again, or `None` if there
    /// is no such radio.
    pub async fn waits(&self, id: RadioId) -> Result<Option<Waits>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        let at = now();
        Ok(Some(Waits {
            position: self.wait(id, Pace::Position, at).await?,
            telemetry: self.wait(id, Pace::Telemetry, at).await?,
            traceroute: self.wait(id, Pace::Traceroute, at).await?,
        }))
    }

    /// The requests sent through a radio, the newest first, or `None` if
    /// there is no such radio.
    pub async fn requests(
        &self,
        id: RadioId,
        node: Option<u32>,
        kind: Option<RequestKind>,
        before: Option<meshctl_api::RequestId>,
        limit: u32,
    ) -> Result<Option<Vec<Request>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(
            self.store.requests(id, node, kind, before, limit).await?,
        ))
    }

    /// A radio's own settings, as it sent them on syncing. Only while it is
    /// connected: they are not kept, and a radio that is not there cannot be
    /// changed anyway.
    /// Whose settings, as its pages show them: a node's asked of it, all
    /// of what they show, if meshctl has not them yet, or anew if `fresh`.
    pub async fn settings(
        &self,
        id: RadioId,
        whose: Whose,
        fresh: bool,
    ) -> Result<NodeSettings, SettingsFailure> {
        let mut settings = self
            .with_settings(id, whose, &NODE_PARTS, fresh, crate::settings::view)
            .await?
            .ok_or(SettingsFailure::NotConnected)?;
        if whose != Whose::Radio {
            return Ok(settings);
        }
        // Whether each channel is silenced, as meshctl has it, which the
        // radio is set to.
        for channel in &mut settings.channels {
            let peer = Peer::Channel {
                channel: channel.index,
            };
            channel.muted = self
                .store
                .muted(id, peer)
                .await
                .map_err(SettingsFailure::Failed)?;
        }
        Ok(settings)
    }

    /// A channel's key, for the one who asks for it, and only then: nothing
    /// else hands it out. Each time is logged.
    pub async fn channel_secret(
        &self,
        id: RadioId,
        whose: Whose,
        index: u32,
    ) -> Result<Option<ChannelSecret>, SettingsFailure> {
        let (sent, secret) = self
            .with_settings(id, whose, &NODE_PARTS, false, |settings| {
                (
                    crate::settings::view(settings).is_some(),
                    crate::settings::secret(settings, index),
                )
            })
            .await?;
        // As for its settings: not before the radio has sent them.
        if !sent {
            return Err(SettingsFailure::NotConnected);
        }
        if secret.is_some() {
            tracing::info!(
                radio = id,
                ?whose,
                channel = index,
                "channel key handed out"
            );
        }
        Ok(secret)
    }

    /// Puts a radio's channels in `order`, by the slots they have now: the
    /// first becomes the primary, the rest follow it with no gap, and any
    /// left out is taken off the radio. Each slot is given its conversation at
    /// once, not only once the radio is read back; that of one left out
    /// waits for it, in no slot, unless `forget`.
    async fn rearrange_channels(
        &self,
        id: RadioId,
        whose: Whose,
        order: &[u32],
        forget: bool,
    ) -> Result<SettingsApplied, SettingsFailure> {
        use crate::channels::{rearranged, slots};
        let (sent, before, after) = self
            .with_settings(id, whose, &NODE_PARTS, false, |current| {
                let sent = rearranged(current, order)?;
                let mut settings = current.clone();
                for channel in &sent {
                    settings
                        .channels
                        .insert(channel.index as u32, channel.clone());
                }
                Ok((sent, slots(current), slots(&settings)))
            })
            .await?
            .map_err(|why: crate::settings::Invalid| SettingsFailure::Invalid(why.0.to_owned()))?;
        if !sent.is_empty() {
            self.apply(
                id,
                whose,
                meshctl_core::radio::SettingsChange {
                    channels: sent,
                    ..Default::default()
                },
            )
            .await?;
        }
        // Another node's channels have no conversations here.
        if whose != Whose::Radio {
            tracing::info!(radio = id, ?whose, ?order, "channels rearranged");
            return Ok(SettingsApplied { reboots: false });
        }
        // Should the radio not take it, reading it back puts each
        // conversation back where it was.
        let followed = async {
            // Before the slots are taken anew: it goes by the one it has.
            if forget {
                for gone in before.iter().filter(|b| !order.contains(&b.index)) {
                    self.store.forget_channel(id, gone.index).await?;
                }
            }
            self.store.follow_channels(id, &after, now()).await?;
            anyhow::Ok(())
        };
        if let Err(err) = followed.await {
            tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
        }
        tracing::info!(radio = id, ?order, forget, "channels rearranged");
        Ok(SettingsApplied { reboots: false })
    }

    /// Puts a radio's channels in another order: every one it has, by its
    /// slot now.
    pub async fn reorder_channels(
        &self,
        id: RadioId,
        whose: Whose,
        order: &[u32],
    ) -> Result<SettingsApplied, SettingsFailure> {
        let have = self
            .with_settings(id, whose, &NODE_PARTS, false, |settings| {
                crate::channels::slots(settings).len()
            })
            .await?;
        if order.len() != have {
            return Err(SettingsFailure::Invalid("order".to_owned()));
        }
        self.rearrange_channels(id, whose, order, false).await
    }

    /// Takes a channel off a radio, but its primary: those after it move up
    /// a slot. Its conversation waits for it, in no slot, unless `forget`.
    pub async fn remove_channel(
        &self,
        id: RadioId,
        whose: Whose,
        index: u32,
        forget: bool,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let have: Vec<u32> = self
            .with_settings(id, whose, &NODE_PARTS, false, |settings| {
                crate::channels::slots(settings)
                    .iter()
                    .map(|s| s.index)
                    .collect()
            })
            .await?;
        if index == 0 || !have.contains(&index) {
            return Err(SettingsFailure::Invalid("channel".to_owned()));
        }
        let order: Vec<u32> = have.into_iter().filter(|&i| i != index).collect();
        self.rearrange_channels(id, whose, &order, forget).await
    }

    /// A secret of one of a radio's sections, for the one who asks for it, and
    /// only then. Each time is logged.
    pub async fn section_secret(
        &self,
        id: RadioId,
        whose: Whose,
        section: &str,
        field: &str,
    ) -> Result<Option<SectionSecret>, SettingsFailure> {
        let value = self
            .with_settings(id, whose, &[section], false, |settings| {
                crate::sections::secret(settings, section, field)
            })
            .await?;
        if value.is_some() {
            tracing::info!(
                radio = id,
                ?whose,
                section,
                field,
                "setting's secret handed out"
            );
        }
        Ok(value.map(|value| SectionSecret {
            section: section.to_owned(),
            field: field.to_owned(),
            value,
        }))
    }

    /// A radio whose settings can be read and changed now: there is such a
    /// radio, meshctl knows its settings, and it is connected.
    fn configurable(&self, id: RadioId) -> Result<Mesh, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.protocol(id) != Some(Protocol::Meshtastic) {
            return Err(SettingsFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(SettingsFailure::NotConnected);
        }
        Ok(mesh)
    }

    /// Reads whose settings: the radio's own, as it sent them; a node's,
    /// with those `parts` of them asked of it first, over the mesh as its
    /// admin, if meshctl has not them yet, or anew if `fresh`.
    async fn with_settings<R>(
        &self,
        id: RadioId,
        whose: Whose,
        parts: &[&str],
        fresh: bool,
        f: impl FnOnce(&meshctl_core::mesh::Settings) -> R,
    ) -> Result<R, SettingsFailure> {
        let mesh = self.configurable(id)?;
        let Whose::Node(node) = whose else {
            return Ok(mesh.read(|state| f(&state.settings)));
        };
        for part in parts {
            let had = self
                .remote
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(id, node))
                .is_some_and(|settings| has_part(settings, part));
            if had && !fresh {
                continue;
            }
            let request = meshctl_core::mesh::settings_request(part)
                .ok_or_else(|| SettingsFailure::Invalid((*part).to_owned()))?;
            let answer = mesh
                .ask_node(node, request)
                .await
                .map_err(node_admin_failure)?;
            self.remote
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry((id, node))
                .or_default()
                .keep_answer(&answer);
        }
        let remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        let none = meshctl_core::mesh::Settings::default();
        Ok(f(remote.get(&(id, node)).unwrap_or(&none)))
    }

    /// Sends a change of whose settings: to the radio, which is read back;
    /// to a node, over the mesh as its admin, and kept as it then has them.
    async fn apply(
        &self,
        id: RadioId,
        whose: Whose,
        change: meshctl_core::radio::SettingsChange,
    ) -> Result<(), SettingsFailure> {
        let mesh = self.configurable(id)?;
        let Whose::Node(node) = whose else {
            return mesh.change_settings(change).await.map_err(admin_failure);
        };
        mesh.change_node(node, change.clone())
            .await
            .map_err(node_admin_failure)?;
        let mut remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        let settings = remote.entry((id, node)).or_default();
        if let Some(owner) = change.owner {
            settings.owner = Some(owner);
        }
        for config in &change.configs {
            settings.keep(config);
        }
        for module in &change.module_configs {
            settings.keep_module(module);
        }
        for channel in change.channels {
            settings.channels.insert(channel.index as u32, channel);
        }
        Ok(())
    }

    /// A connected MeshCore radio.
    fn meshcore(&self, id: RadioId) -> Result<Mesh, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.protocol(id) != Some(Protocol::Meshcore) {
            return Err(SettingsFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(SettingsFailure::NotConnected);
        }
        Ok(mesh)
    }

    /// A MeshCore radio's own settings, as it last said them.
    pub fn meshcore_settings(
        &self,
        id: RadioId,
    ) -> Result<meshctl_api::MeshcoreSettings, SettingsFailure> {
        let mesh = self.meshcore(id)?;
        mesh.read(|state| state.meshcore.as_ref().map(crate::meshcore::view))
            .ok_or(SettingsFailure::NotConnected)
    }

    /// Changes a MeshCore radio's own settings, each as the radio applies
    /// it, at once; read back from it after.
    pub async fn change_meshcore_settings(
        &self,
        id: RadioId,
        wanted: &meshctl_api::MeshcoreSettings,
    ) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let current = mesh
            .read(|state| state.meshcore.clone())
            .ok_or(SettingsFailure::NotConnected)?;
        let change = crate::meshcore::change(&current, wanted).map_err(SettingsFailure::Invalid)?;
        mesh.meshcore_change(&change)
            .await
            .map_err(|error| match error {
                // What the radio would not take, as it names it.
                AdminError::Refused(why) => SettingsFailure::Invalid(why),
                other => admin_failure(other),
            })?;
        tracing::info!(radio = id, "MeshCore settings changed");
        Ok(())
    }

    /// A MeshCore radio's channels, each by its kind.
    pub async fn meshcore_channels(
        &self,
        id: RadioId,
    ) -> Result<meshctl_api::MeshcoreChannels, SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let scopes = self
            .store
            .channel_scopes(id)
            .await
            .map_err(SettingsFailure::Failed)?;
        mesh.read(|state| crate::meshcore::channels(state, &scopes))
            .ok_or(SettingsFailure::NotConnected)
    }

    /// Keeps the region a MeshCore channel floods in, by the channel: its
    /// name as kept and its secret.
    async fn keep_scope(
        &self,
        id: RadioId,
        name: &str,
        secret: &[u8; 16],
        scope: Option<&str>,
    ) -> Result<(), SettingsFailure> {
        let print = crate::channels::fingerprint(name, secret);
        self.store
            .set_channel_scope(id, &print, scope)
            .await
            .map_err(SettingsFailure::Failed)
    }

    /// Sets a MeshCore radio's channel in a slot: its name, and its secret
    /// as given, as its name gives it, as it was, or a new one.
    pub async fn set_meshcore_channel(
        &self,
        id: RadioId,
        index: u32,
        wanted: &meshctl_api::MeshcoreChannelChange,
    ) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let (name, secret) = mesh
            .read(|state| {
                crate::meshcore::channel_change(state, index, wanted, |secret| {
                    getrandom::fill(secret).expect("the system has randomness to give");
                })
            })
            .map_err(|why| match why {
                crate::meshcore::DUPLICATE => SettingsFailure::Duplicate,
                why => SettingsFailure::Invalid(why.to_owned()),
            })?;
        let scope = match wanted.scope.as_deref() {
            Some(scope) => Some(
                crate::meshcore::scope_name(scope)
                    .ok_or_else(|| SettingsFailure::Invalid("scope".into()))?,
            ),
            None => None,
        };
        let index = u8::try_from(index).map_err(|_| SettingsFailure::Invalid("index".into()))?;
        // Kept first: the radio is read back as it takes it, and the channel
        // is shown with its region then.
        self.keep_scope(id, &name, &secret, scope.as_deref())
            .await?;
        mesh.meshcore_set_channel(index, &name, &secret)
            .await
            .map_err(meshcore_failure)?;
        tracing::info!(radio = id, index, "MeshCore channel set");
        Ok(())
    }

    /// Puts a MeshCore radio's channels in another order: every one it has,
    /// by its slot now, to the first slots.
    pub async fn reorder_meshcore_channels(
        &self,
        id: RadioId,
        order: &[u32],
    ) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let channels = mesh
            .read(|state| {
                let meshcore = state.meshcore_channels.as_ref()?;
                let mut seen = std::collections::BTreeSet::new();
                let picked: Option<Vec<_>> = order
                    .iter()
                    .map(|index| {
                        let channel = state.channels.get(index)?;
                        seen.insert(*index).then_some(())?;
                        Some((channel.name.clone(), *meshcore.secrets.get(index)?))
                    })
                    .collect();
                picked.filter(|p| p.len() == state.channels.len())
            })
            .ok_or_else(|| SettingsFailure::Invalid("order".into()))?;
        mesh.meshcore_reorder_channels(&channels)
            .await
            .map_err(meshcore_failure)?;
        tracing::info!(radio = id, ?order, "MeshCore channels reordered");
        Ok(())
    }

    /// Keeps the region a MeshCore channel in a slot floods in, or none for
    /// the radio's default: for the channel, nothing written to the radio.
    pub async fn set_meshcore_channel_scope(
        &self,
        id: RadioId,
        index: u32,
        scope: Option<&str>,
    ) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let scope = match scope {
            Some(scope) => Some(
                crate::meshcore::scope_name(scope)
                    .ok_or_else(|| SettingsFailure::Invalid("scope".into()))?,
            ),
            None => None,
        };
        let (name, secret) = mesh
            .read(|state| {
                let channel = state.channels.get(&index)?;
                let secret = state.meshcore_channels.as_ref()?.secrets.get(&index)?;
                Some((channel.name.clone(), *secret))
            })
            .ok_or_else(|| SettingsFailure::Invalid("index".into()))?;
        self.keep_scope(id, &name, &secret, scope.as_deref())
            .await?;
        // Whoever shows the channels reads them again.
        let _ = self.changes.send(Change::Mesh(id, mesh::Event::Synced));
        tracing::info!(radio = id, index, ?scope, "MeshCore channel's region set");
        Ok(())
    }

    /// Empties a MeshCore radio's channel slot. Its conversation stays,
    /// as a channel gone.
    pub async fn remove_meshcore_channel(
        &self,
        id: RadioId,
        index: u32,
    ) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let set = mesh.read(|state| state.channels.contains_key(&index));
        if !set {
            return Err(SettingsFailure::Invalid("index".into()));
        }
        let index = u8::try_from(index).map_err(|_| SettingsFailure::Invalid("index".into()))?;
        mesh.meshcore_set_channel(index, "", &[0; 16])
            .await
            .map_err(meshcore_failure)?;
        tracing::info!(radio = id, index, "MeshCore channel removed");
        Ok(())
    }

    /// The regions a MeshCore radio's repeaters and rooms said they repeat
    /// on, each as it last said; and whether it is asking them now.
    pub async fn meshcore_regions(
        &self,
        id: RadioId,
    ) -> Result<meshctl_api::MeshcoreRegions, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.protocol(id) != Some(Protocol::Meshcore) {
            return Err(SettingsFailure::Unsupported);
        }
        let said = self
            .store
            .latest_regions(id)
            .await
            .map_err(SettingsFailure::Failed)?;
        let left = self
            .searching
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .copied();
        let repeaters: Vec<meshctl_api::MeshcoreRepeaterRegions> = mesh.read(|state| {
            said.into_iter()
                .filter_map(|(node, reply, answered_at)| {
                    let meshctl_api::RequestReply::Regions {
                        regions, unscoped, ..
                    } = reply
                    else {
                        return None;
                    };
                    let known = state.nodes.get(&node);
                    Some(meshctl_api::MeshcoreRepeaterRegions {
                        node,
                        name: known
                            .and_then(|n| n.user.as_ref())
                            .map(|u| u.long_name.clone()),
                        hops: known.and_then(|n| n.hops_away),
                        regions,
                        unscoped,
                        answered_at,
                    })
                })
                .collect()
        });
        Ok(crate::meshcore::gather_regions(left, repeaters))
    }

    /// Asks a MeshCore radio's repeaters and rooms their regions, those it
    /// knows a path to, the nearest first: one at a time, each waited for
    /// and spaced out, in the background. Those that answered lately are
    /// not asked again. How things stand as it starts.
    pub async fn search_regions(
        &self,
        id: RadioId,
    ) -> Result<meshctl_api::MeshcoreRegions, SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let lately: std::collections::HashSet<u32> = self
            .store
            .latest_regions(id)
            .await
            .map_err(SettingsFailure::Failed)?
            .into_iter()
            .filter(|(.., at)| now() - at < REGIONS_FRESH)
            .map(|(node, ..)| node)
            .collect();
        let mut candidates: Vec<(u32, u32, u32)> = mesh.read(|state| {
            let me = state.local.as_ref().map(|l| l.num);
            state
                .nodes
                .values()
                .filter(|n| Some(n.num) != me && n.on_radio != Some(false))
                .filter(|n| {
                    n.user
                        .as_ref()
                        .is_some_and(|u| u.role == "REPEATER" || u.role == "ROOM")
                })
                .filter_map(|n| {
                    let hops = n.hops_away?;
                    (hops <= REGIONS_HOPS && !lately.contains(&n.num)).then_some((
                        hops,
                        u32::MAX - n.last_heard.unwrap_or(0),
                        n.num,
                    ))
                })
                .collect()
        });
        candidates.sort_unstable();
        candidates.truncate(REGIONS_MOST);
        let nodes: Vec<u32> = candidates.into_iter().map(|(.., num)| num).collect();
        // One search at a time: another while one goes on only says how it
        // goes.
        let started = {
            let mut searching = self.searching.lock().unwrap_or_else(|e| e.into_inner());
            let free = !searching.contains_key(&id) && !nodes.is_empty();
            if free {
                searching.insert(id, u32::try_from(nodes.len()).unwrap_or(u32::MAX));
            }
            free
        };
        if started {
            tracing::info!(radio = id, repeaters = nodes.len(), "searching for regions");
            let radios = self.clone();
            tokio::spawn(async move { radios.ask_regions(id, nodes).await });
        }
        self.meshcore_regions(id).await
    }

    /// Asks each node its regions in turn, each answer waited for, then a
    /// pause, as repeaters keep from answering many anonymous requests.
    async fn ask_regions(&self, id: RadioId, nodes: Vec<u32>) {
        let mut changes = self.changes.subscribe();
        let total = nodes.len();
        for (done, node) in nodes.into_iter().enumerate() {
            let asked = self
                .request(
                    id,
                    NewRequest {
                        node,
                        kind: RequestKind::Regions,
                        channel: 0,
                        seconds: None,
                    },
                )
                .await;
            match asked {
                Ok(request) => {
                    // Its answer, or giving up on it.
                    let _ = tokio::time::timeout(REGIONS_WAIT, async {
                        loop {
                            match changes.recv().await {
                                Ok(Change::Request(radio, r))
                                    if radio == id
                                        && r.id == request.id
                                        && r.status != meshctl_api::RequestStatus::Pending =>
                                {
                                    return;
                                }
                                Err(broadcast::error::RecvError::Closed) => return,
                                _ => {}
                            }
                        }
                    })
                    .await;
                }
                Err(AskFailure::NotConnected | AskFailure::NoSuchRadio) => break,
                Err(why) => tracing::debug!(radio = id, node, ?why, "not asked its regions"),
            }
            let left = u32::try_from(total - done - 1).unwrap_or(0);
            self.searching
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, left);
            if left > 0 {
                tokio::time::sleep(REGIONS_PAUSE).await;
            }
        }
        self.searching
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        tracing::info!(radio = id, "regions searched");
        // Whoever shows them reads them again.
        let _ = self.changes.send(Change::Mesh(id, mesh::Event::Synced));
    }

    /// A channel's link, to share it, as its radio's apps read it: a
    /// Meshtastic one to be added, a MeshCore one with its region.
    pub async fn channel_link(
        &self,
        id: RadioId,
        whose: Whose,
        index: u32,
    ) -> Result<String, SettingsFailure> {
        if whose == Whose::Radio && self.protocol(id) == Some(Protocol::Meshcore) {
            return self.meshcore_channel_link(id, index).await;
        }
        self.with_settings(id, whose, &NODE_PARTS, false, |settings| {
            crate::links::channel_link(settings, index)
        })
        .await?
        .ok_or(SettingsFailure::Invalid("index".into()))
    }

    /// A MeshCore channel's link, to share it: with its secret, and its
    /// region if it has one.
    pub async fn meshcore_channel_link(
        &self,
        id: RadioId,
        index: u32,
    ) -> Result<String, SettingsFailure> {
        let channels = self.meshcore_channels(id).await?;
        let scope = channels
            .channels
            .iter()
            .find(|c| c.index == index)
            .and_then(|c| c.scope.clone());
        let mesh = self.meshcore(id)?;
        mesh.read(|state| crate::meshcore::channel_link(state, index, scope.as_deref()))
            .ok_or(SettingsFailure::Invalid("index".into()))
    }

    /// Has a MeshCore radio forget its path to a node.
    pub async fn reset_path(&self, id: RadioId, num: u32) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        mesh.meshcore_reset_path(num)
            .await
            .map_err(meshcore_failure)
    }

    /// Has a MeshCore radio keep a node it does not, which it was heard of.
    pub async fn keep_contact(&self, id: RadioId, num: u32) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        mesh.meshcore_keep(num).await.map_err(meshcore_failure)
    }

    /// Has a MeshCore radio say who it is.
    pub async fn advert(&self, id: RadioId, flood: bool) -> Result<(), SettingsFailure> {
        let mesh = self.meshcore(id)?;
        mesh.meshcore_advert(flood).await.map_err(meshcore_failure)
    }

    /// A MeshCore radio's figures, asked for now.
    pub async fn meshcore_status(
        &self,
        id: RadioId,
    ) -> Result<meshctl_api::MeshcoreStatus, SettingsFailure> {
        let mesh = self.meshcore(id)?;
        let status = mesh.meshcore_status().await.map_err(admin_failure)?;
        Ok(crate::meshcore::status(status))
    }

    /// What a link holds, against what the radio has and knows: a
    /// Meshtastic link for a Meshtastic radio, a MeshCore one for MeshCore.
    pub fn read_link(&self, id: RadioId, url: &str) -> Result<LinkContents, LinkFailure> {
        if self.protocol(id) == Some(Protocol::Meshcore) {
            let mesh = self.meshcore(id).map_err(LinkFailure::Settings)?;
            let link = crate::meshcore::parse_link(url).ok_or(LinkFailure::Invalid)?;
            return Ok(mesh.read(|state| crate::meshcore::link_contents(&link, state)));
        }
        let mesh = self.configurable(id).map_err(LinkFailure::Settings)?;
        let parsed = crate::links::parse(url).ok_or(LinkFailure::Invalid)?;
        Ok(mesh.read(|state| crate::links::contents(&parsed, &state.settings, &state.nodes)))
    }

    /// Adds what a MeshCore link holds: a contact the radio keeps, or a
    /// channel to its first free slot, unless it has it already.
    async fn add_meshcore_link(&self, id: RadioId, url: &str) -> Result<(), LinkFailure> {
        let mesh = self.meshcore(id).map_err(LinkFailure::Settings)?;
        match crate::meshcore::parse_link(url).ok_or(LinkFailure::Invalid)? {
            crate::meshcore::Link::Contact { key, kind, name } => mesh
                .meshcore_add_contact(key, kind, &name)
                .await
                .map(|_| ())
                .map_err(LinkFailure::Admin),
            crate::meshcore::Link::Channel {
                name,
                secret,
                scope,
            } => {
                // Its region kept with it, whether the radio had it or not.
                if scope.is_some() {
                    self.keep_scope(id, &name, &secret, scope.as_deref())
                        .await
                        .map_err(LinkFailure::Settings)?;
                }
                let (present, free) = mesh.read(|state| {
                    (
                        crate::meshcore::has_channel(state, &name, &secret),
                        crate::meshcore::free_slot(state),
                    )
                });
                if present {
                    return Ok(());
                }
                let slot = free.ok_or(LinkFailure::NoFreeSlots)?;
                mesh.meshcore_set_channel(slot, &name, &secret)
                    .await
                    .map_err(LinkFailure::Admin)
            }
        }
    }

    /// Adds what a link holds to a Meshtastic radio: a contact it keeps, or
    /// channels to its free slots, those it has left out. Nothing else of
    /// its settings changes, its LoRa least of all.
    pub async fn add_link(&self, id: RadioId, link: &meshctl_api::Link) -> Result<(), LinkFailure> {
        if self.protocol(id) == Some(Protocol::Meshcore) {
            return self.add_meshcore_link(id, &link.url).await;
        }
        let mesh = self.configurable(id).map_err(LinkFailure::Settings)?;
        match crate::links::parse(&link.url).ok_or(LinkFailure::Invalid)? {
            crate::links::Parsed::Contact(contact) => {
                mesh.add_contact(contact).await.map_err(LinkFailure::Admin)
            }
            crate::links::Parsed::Channels(set, _) => {
                let wanted = match link.mode {
                    meshctl_api::LinkMode::Add => {
                        let channels = mesh
                            .read(|state| crate::links::channel_changes(&set, &state.settings))
                            .ok_or(LinkFailure::NoFreeSlots)?;
                        if channels.is_empty() {
                            return Ok(());
                        }
                        NodeSettingsChange {
                            channels,
                            ..Default::default()
                        }
                    }
                    // Every one, and the LoRa settings only when asked.
                    meshctl_api::LinkMode::Replace => NodeSettingsChange {
                        channels: crate::links::replacing_changes(&set),
                        lora: if link.lora {
                            mesh.read(|state| crate::links::lora_settings(&set, &state.settings))
                        } else {
                            None
                        },
                        ..Default::default()
                    },
                };
                self.change_settings(id, Whose::Radio, &wanted)
                    .await
                    .map(|_| ())
                    .map_err(LinkFailure::Settings)
            }
        }
    }

    /// Changes a radio's own settings. Returns whether it reboots to apply
    /// them.
    pub async fn change_settings(
        &self,
        id: RadioId,
        whose: Whose,
        wanted: &NodeSettingsChange,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let (change, reboots) = self
            .with_settings(id, whose, &NODE_PARTS, false, |settings| {
                crate::settings::change(settings, wanted, |key| {
                    getrandom::fill(key).expect("the system has randomness to give");
                })
            })
            .await?
            .map_err(|why| match why.0 {
                crate::settings::DUPLICATE => SettingsFailure::Duplicate,
                crate::settings::PUBLIC_PRECISION => SettingsFailure::PublicPrecision,
                why => SettingsFailure::Invalid(why.to_owned()),
            })?;
        self.apply(id, whose, change).await?;
        tracing::info!(radio = id, ?whose, reboots, "settings changed");
        // Another node's channels have no conversations here to silence.
        if whose != Whose::Radio {
            return Ok(SettingsApplied { reboots });
        }
        // A channel silenced, or no longer, is so in meshctl, which says.
        for channel in &wanted.channels {
            if let Some(muted) = channel.muted {
                let peer = Peer::Channel {
                    channel: channel.index,
                };
                if self
                    .store
                    .set_muted(id, peer, muted)
                    .await
                    .map_err(SettingsFailure::Failed)?
                {
                    let _ = self.changes.send(Change::ConversationMuted {
                        radio: id,
                        peer,
                        muted,
                    });
                }
            }
        }
        Ok(SettingsApplied { reboots })
    }

    /// The rest of a radio's configuration and its modules, section by
    /// section; like its settings, only while it is connected.
    /// The sections of a node's settings it can be asked for over the mesh,
    /// as its admin, by their ids: what it says it has, asked of it, which
    /// tells too whether it takes the radio's key as an admin key at all.
    pub async fn node_sections(
        &self,
        id: RadioId,
        node: u32,
    ) -> Result<Vec<String>, SettingsFailure> {
        let mesh = self.configurable(id)?;
        let request = meshctl_core::mesh::settings_request("metadata").expect("a request");
        let answer = mesh
            .ask_node(node, request)
            .await
            .map_err(node_admin_failure)?;
        let mut remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        let settings = remote.entry((id, node)).or_default();
        settings.keep_answer(&answer);
        Ok(crate::sections::offered(settings))
    }

    /// One section of a node's settings, asked of it over the mesh as its
    /// admin, as the radio's own are shown.
    pub async fn node_section(
        &self,
        id: RadioId,
        node: u32,
        section: &str,
    ) -> Result<SettingsSection, SettingsFailure> {
        let mesh = self.configurable(id)?;
        let offered = crate::sections::offered(&meshctl_core::mesh::Settings::default());
        let request = offered
            .iter()
            .any(|s| s == section)
            .then(|| meshctl_core::mesh::settings_request(section))
            .flatten()
            .ok_or_else(|| SettingsFailure::Invalid(section.to_owned()))?;
        let answer = mesh
            .ask_node(node, request)
            .await
            .map_err(node_admin_failure)?;
        let mut remote = self.remote.lock().unwrap_or_else(|e| e.into_inner());
        let settings = remote.entry((id, node)).or_default();
        if !settings.keep_answer(&answer) {
            return Err(SettingsFailure::NoAnswer);
        }
        crate::sections::sections(settings)
            .into_iter()
            .find(|s| s.id == section)
            .ok_or(SettingsFailure::NoAnswer)
    }

    /// Has whose node do something, as its admin: the radio itself, or
    /// another node over the mesh.
    pub async fn act(
        &self,
        id: RadioId,
        whose: Whose,
        action: meshctl_api::NodeAction,
    ) -> Result<(), SettingsFailure> {
        use meshctl_api::NodeAction as A;
        use meshctl_core::radio::NodeAction as N;
        let mesh = self.configurable(id)?;
        let action = match action {
            A::Reboot => N::Reboot,
            A::Shutdown => N::Shutdown,
            A::ForgetNodes => N::ForgetNodes,
            A::FactoryReset => N::FactoryReset,
        };
        match whose {
            Whose::Radio => mesh.act(None, action).await.map_err(admin_failure)?,
            Whose::Node(node) => {
                mesh.act(Some(node), action)
                    .await
                    .map_err(node_admin_failure)?;
                // What meshctl had of it is no longer so.
                if matches!(action, N::FactoryReset) {
                    self.remote
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&(id, node));
                }
            }
        }
        tracing::info!(radio = id, ?whose, ?action, "node told to act");
        Ok(())
    }

    /// The keys a radio takes admin requests from over the mesh.
    pub async fn admin_keys(
        &self,
        id: RadioId,
        whose: Whose,
    ) -> Result<meshctl_api::AdminKeys, SettingsFailure> {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64;
        self.with_settings(id, whose, &[SECURITY], true, |settings| {
            let security = settings
                .config
                .security
                .as_ref()
                .ok_or(SettingsFailure::NotConnected)?;
            Ok(meshctl_api::AdminKeys {
                keys: security
                    .admin_key
                    .iter()
                    .map(|k| BASE64.encode(k))
                    .collect(),
                managed: security.is_managed,
            })
        })
        .await?
    }

    /// Sets the keys a radio takes admin requests from: the rest of its
    /// security as it has it, its own keys and managed mode untouched. Up
    /// to three, of 32 bytes each, once each; and, a managed radio, at
    /// least one, or nothing could change it but a cable.
    pub async fn change_admin_keys(
        &self,
        id: RadioId,
        whose: Whose,
        keys: &[String],
    ) -> Result<SettingsApplied, SettingsFailure> {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64;
        let mut decoded: Vec<Vec<u8>> = Vec::with_capacity(keys.len());
        for key in keys {
            let bytes = BASE64
                .decode(key.trim())
                .ok()
                .filter(|b| b.len() == 32)
                .ok_or_else(|| SettingsFailure::Invalid("admin_key".into()))?;
            if decoded.contains(&bytes) {
                return Err(SettingsFailure::Invalid("admin_key".into()));
            }
            decoded.push(bytes);
        }
        if decoded.len() > 3 {
            return Err(SettingsFailure::Invalid("admin_key".into()));
        }
        let security = self
            .with_settings(id, whose, &[SECURITY], false, |settings| {
                settings.config.security.clone()
            })
            .await?
            .ok_or(SettingsFailure::NotConnected)?;
        if security.is_managed && decoded.is_empty() {
            return Err(SettingsFailure::Invalid("admin_key".into()));
        }
        let security = meshctl_proto::meshtastic::config::SecurityConfig {
            admin_key: decoded,
            ..security
        };
        let change = meshctl_core::radio::SettingsChange {
            configs: vec![meshctl_proto::meshtastic::Config {
                payload_variant: Some(meshctl_proto::meshtastic::config::PayloadVariant::Security(
                    security,
                )),
            }],
            ..Default::default()
        };
        self.apply(id, whose, change).await?;
        tracing::info!(radio = id, ?whose, keys = keys.len(), "admin keys changed");
        Ok(SettingsApplied { reboots: true })
    }

    pub fn sections(&self, id: RadioId) -> Result<Vec<SettingsSection>, SettingsFailure> {
        Ok(self
            .configurable(id)?
            .read(|state| crate::sections::sections(&state.settings)))
    }

    /// Changes some settings of one section, and where the radio is, fixed,
    /// if given. The firmware reboots for nearly any setting, so it is
    /// always said that it does; a fixed position moved alone it takes as
    /// it is.
    pub async fn change_section(
        &self,
        id: RadioId,
        whose: Whose,
        section: &str,
        values: &BTreeMap<String, serde_json::Value>,
        at: Option<&FixedPosition>,
    ) -> Result<SettingsApplied, SettingsFailure> {
        if values.is_empty() && at.is_none() {
            return Err(SettingsFailure::Invalid(section.to_owned()));
        }
        let change = self
            .with_settings(id, whose, &[section], false, |settings| {
                let mut change = if values.is_empty() {
                    meshctl_core::radio::SettingsChange::default()
                } else {
                    crate::sections::change(settings, section, values)?
                };
                change.fixed_position = crate::sections::fixed(settings, section, values, at)?;
                Ok(change)
            })
            .await?
            .map_err(|why: crate::sections::Invalid| SettingsFailure::Invalid(why.0))?;
        self.apply(id, whose, change).await?;
        tracing::info!(radio = id, ?whose, section, "settings changed");
        Ok(SettingsApplied {
            reboots: !values.is_empty(),
        })
    }

    /// The packets a radio heard, the newest first, or `None` if there is no
    /// such radio.
    /// The packet that carried one of a radio's messages, as the radio heard
    /// or sent it: `None` if it kept no such hearing.
    pub async fn message_packet(&self, id: RadioId, message: MessageId) -> Result<Option<Packet>> {
        Ok(self
            .store
            .message_sighting(id, message)
            .await?
            .map(crate::packets::packet))
    }

    pub async fn packets(
        &self,
        id: RadioId,
        before: Option<SightingId>,
        filter: &PacketFilter,
        limit: u32,
    ) -> Result<Option<Vec<Packet>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        // A MeshCore radio's are its own, raw.
        if self.protocol(id) == Some(Protocol::Meshcore) {
            return Ok(Some(
                self.store
                    .meshcore_packets(id, before, filter, limit)
                    .await?,
            ));
        }
        let rows = self.store.packets(id, before, filter, limit).await?;
        Ok(Some(rows.into_iter().map(crate::packets::packet).collect()))
    }

    pub async fn list(&self) -> Result<Vec<Radio>> {
        let records = self.store.radios().await?;
        let mut radios = Vec::with_capacity(records.len());
        for record in records {
            let mut radio = self.radio(record);
            radio.unread = self.store.unread(radio.id).await?;
            radios.push(radio);
        }
        Ok(radios)
    }

    pub async fn get(&self, id: RadioId) -> Result<Option<Radio>> {
        let Some(record) = self.store.radio(id).await? else {
            return Ok(None);
        };
        let mut radio = self.radio(record);
        radio.unread = self.store.unread(id).await?;
        Ok(Some(radio))
    }

    /// Adds a radio, already validated, and connects to it if it is enabled.
    pub async fn add(&self, settings: RadioSettings) -> Result<Radio> {
        let record = self.store.add_radio(&settings).await?;
        self.run(&record, MeshState::default());
        self.changed();
        Ok(self.radio(record))
    }

    /// Changes a radio, already validated, reconnecting to it as it is now
    /// when what it connects to, or whether it does, changed.
    pub async fn update(&self, id: RadioId, settings: RadioSettings) -> Result<Option<Radio>> {
        let before = self.store.radio(id).await?;
        if !self.store.update_radio(id, &settings).await? {
            return Ok(None);
        }
        let Some(record) = self.store.radio(id).await? else {
            return Ok(None);
        };
        // How it shows alone changes nothing of its link.
        let link = |settings: &RadioSettings| {
            (
                settings.name.clone(),
                settings.kind,
                settings.address.clone(),
                settings.enabled,
            )
        };
        if before.is_some_and(|before| link(&before.settings) == link(&record.settings))
            && self.lock().contains_key(&id)
        {
            self.changed();
            return Ok(Some(self.radio(record)));
        }
        // What it knew carries over, more current than what was stored.
        let previous = self.lock().remove(&id);
        let state = match previous {
            Some(running) => running.mesh.read(MeshState::clone),
            None => self.stored_state(id).await?,
        };
        // Its link goes with the old settings: it was connected until now.
        let record = if state.link == mesh::LinkState::Ready {
            let at = now();
            self.store.set_last_connected(id, at).await?;
            RadioRecord {
                last_connected: Some(at),
                ..record
            }
        } else {
            record
        };
        self.run(&record, state);
        self.changed();
        Ok(Some(self.radio(record)))
    }

    /// Puts the radios in the order given; `false`, changing nothing, unless
    /// it names each radio there is once.
    pub async fn reorder(&self, radios: &[RadioId]) -> Result<bool> {
        let mut given = radios.to_vec();
        given.sort_unstable();
        let mut there: Vec<RadioId> = self.store.radios().await?.iter().map(|r| r.id).collect();
        there.sort_unstable();
        if given != there {
            return Ok(false);
        }
        self.store.set_radio_order(radios).await?;
        self.changed();
        Ok(true)
    }

    /// Where the map was left for a radio: `None` if there is no such radio.
    /// Nobody is told when it moves: each browser goes on where it is.
    pub async fn map_view(&self, id: RadioId) -> Result<Option<Option<MapView>>> {
        self.store.map_view(id).await
    }

    /// Keeps where the map was left for a radio; `false` if there is none.
    pub async fn set_map_view(&self, id: RadioId, view: &MapView) -> Result<bool> {
        self.store.set_map_view(id, view).await
    }

    /// Keeps how long ago a radio's nodes have to have been heard to be on
    /// its map, and tells every client; `false` if there is no such radio.
    pub async fn set_map_max_age(&self, id: RadioId, max_age: Option<u32>) -> Result<bool> {
        if !self.store.set_map_max_age(id, max_age).await? {
            return Ok(false);
        }
        self.changed();
        Ok(true)
    }

    /// Removes a radio and disconnects from it; `false` if there was none.
    pub async fn remove(&self, id: RadioId) -> Result<bool> {
        if !self.store.remove_radio(id).await? {
            return Ok(false);
        }
        self.lock().remove(&id);
        self.changed();
        Ok(true)
    }

    /// Disconnects from every radio, for good: the daemon is stopping. Those
    /// that were connected until now are stored as such.
    pub async fn stop(&self) {
        self.stopped.send_replace(true);
        let connected: Vec<RadioId> = self
            .lock()
            .iter()
            .filter(|(_, running)| running.mesh.read(|s| s.link) == mesh::LinkState::Ready)
            .map(|(id, _)| *id)
            .collect();
        self.lock().clear();
        let at = now();
        for id in connected {
            if let Err(err) = self.store.set_last_connected(id, at).await {
                tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
            }
        }
    }

    /// The automatic replies, in the order they are tried in.
    pub async fn auto_replies(&self) -> Result<Vec<meshctl_api::AutoReply>> {
        self.store.auto_replies().await
    }

    /// Adds an automatic reply, the last.
    pub async fn add_auto_reply(
        &self,
        settings: &meshctl_api::AutoReplySettings,
    ) -> Result<crate::store::Keeping> {
        let settings = crate::auto_replies::tidy(settings.clone());
        if !crate::auto_replies::valid(&settings) {
            return Ok(crate::store::Keeping::Invalid);
        }
        self.store.add_auto_reply(&settings).await
    }

    /// Changes an automatic reply.
    pub async fn set_auto_reply(
        &self,
        id: meshctl_api::AutoReplyId,
        settings: &meshctl_api::AutoReplySettings,
    ) -> Result<crate::store::Keeping> {
        let settings = crate::auto_replies::tidy(settings.clone());
        if !crate::auto_replies::valid(&settings) {
            return Ok(crate::store::Keeping::Invalid);
        }
        self.store.set_auto_reply(id, &settings).await
    }

    /// What holds every automatic reply back.
    pub async fn auto_reply_limits(&self) -> Result<meshctl_api::AutoReplyLimits> {
        self.store.auto_reply_limits().await
    }

    pub async fn set_auto_reply_limits(&self, limits: &meshctl_api::AutoReplyLimits) -> Result<()> {
        self.store.set_auto_reply_limits(limits).await
    }

    /// Keeps how busy a radio hears its channel now, in percent.
    pub fn set_channel_busy(&self, id: RadioId, percent: f32) {
        self.busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (percent, std::time::Instant::now()));
    }

    /// How busy a radio hears its channel, in percent, if it said so lately.
    pub fn channel_busy(&self, id: RadioId) -> Option<f32> {
        self.busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .filter(|(_, at)| at.elapsed() < BUSY_FRESH)
            .map(|(percent, _)| *percent)
    }

    /// How busy every radio hears its channel, in their order.
    pub async fn channels_busy(&self) -> Result<Vec<meshctl_api::ChannelBusy>> {
        Ok(self
            .store
            .radios()
            .await?
            .into_iter()
            .map(|record| meshctl_api::ChannelBusy {
                radio: record.id,
                percent: self.channel_busy(record.id),
            })
            .collect())
    }

    /// The MeshCore radios connected and ready, to be asked their figures.
    pub fn meshcore_ready(&self) -> Vec<(RadioId, Mesh)> {
        self.lock()
            .iter()
            .filter(|(_, running)| {
                running.protocol == Protocol::Meshcore
                    && running.mesh.read(|s| s.link) == mesh::LinkState::Ready
            })
            .map(|(id, running)| (*id, running.mesh.clone()))
            .collect()
    }

    /// Removes an automatic reply; `false` if there is no such one.
    pub async fn remove_auto_reply(&self, id: meshctl_api::AutoReplyId) -> Result<bool> {
        self.store.remove_auto_reply(id).await
    }

    /// Puts the automatic replies in the order given; `false` unless it
    /// names every one once.
    pub async fn set_auto_reply_order(&self, order: &[meshctl_api::AutoReplyId]) -> Result<bool> {
        self.store.set_auto_reply_order(order).await
    }

    /// Takes the app it runs within, and tells it what it was left set to.
    pub async fn set_host(&self, host: Arc<dyn crate::Host>) {
        let settings = self.store.app_settings().await.unwrap_or_default();
        host.keep_running(settings.background.unwrap_or(true));
        let _ = self.host.set(host);
    }

    /// What the app it runs within lets be set, as it is: nothing without
    /// one.
    pub async fn app_settings(&self) -> Result<meshctl_api::AppSettings> {
        if self.host.get().is_none() {
            return Ok(meshctl_api::AppSettings::default());
        }
        let kept = self.store.app_settings().await?;
        Ok(meshctl_api::AppSettings {
            background: Some(kept.background.unwrap_or(true)),
        })
    }

    /// Changes what the app it runs within lets be set, and tells it;
    /// `false` without one.
    pub async fn set_app_settings(&self, settings: meshctl_api::AppSettings) -> Result<bool> {
        let Some(host) = self.host.get() else {
            return Ok(false);
        };
        let mut kept = self.store.app_settings().await?;
        if let Some(on) = settings.background {
            kept.background = Some(on);
            host.keep_running(on);
        }
        self.store.set_app_settings(&kept).await?;
        Ok(true)
    }

    /// The app it runs within, if any.
    pub fn host(&self) -> Option<Arc<dyn crate::Host>> {
        self.host.get().cloned()
    }

    /// What is notified of, as set, or as it is until set: on only within
    /// an app, and every radio's direct conversations.
    pub async fn notifications(&self) -> Result<meshctl_api::Notifications> {
        let kept = self.store.notifications().await?;
        let places = match kept.places {
            Some(places) => places,
            None => self
                .store
                .radios()
                .await?
                .into_iter()
                .map(|record| meshctl_api::NotificationPlace {
                    place: meshctl_api::AutoReplyPlace::Directs { radio: record.id },
                    mentions: false,
                })
                .collect(),
        };
        Ok(meshctl_api::Notifications {
            enabled: kept.enabled.unwrap_or(self.host.get().is_some()),
            show_text: kept.show_text.unwrap_or(true),
            places,
        })
    }

    /// Sets what is notified of; `false` if a place is on a radio, a
    /// channel or a node there is not.
    pub async fn set_notifications(
        &self,
        notifications: &meshctl_api::Notifications,
    ) -> Result<bool> {
        self.store
            .set_notifications(
                notifications.enabled,
                notifications.show_text,
                &notifications.places,
            )
            .await
    }

    /// Keeps the conversation a client of the event stream shows, and is
    /// seen, or that it shows none.
    pub fn set_viewing(&self, client: u64, viewing: Option<meshctl_api::events::Viewing>) {
        let mut all = self.viewing.lock().unwrap_or_else(|e| e.into_inner());
        match viewing {
            Some(viewing) => all.insert(client, viewing),
            None => all.remove(&client),
        };
    }

    /// Whether a client shows a conversation, and is seen.
    pub fn viewed(&self, radio: RadioId, peer: Peer) -> bool {
        self.viewing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|viewing| viewing.radio == radio && viewing.peer == peer)
    }

    /// Tells every client of the event stream to notify of a message.
    pub fn notify(&self, notification: meshctl_api::Notification) {
        let _ = self.changes.send(Change::Notification(notification));
    }

    /// The name a radio is given in meshctl.
    pub async fn radio_name(&self, radio: RadioId) -> Option<String> {
        self.store
            .radio(radio)
            .await
            .ok()
            .flatten()
            .map(|record| record.settings.name)
    }

    /// The interface's preferences.
    pub async fn preferences(&self) -> Result<Preferences> {
        self.store.preferences().await
    }

    /// The emojis picked in the interface, the most used first.
    pub async fn emojis(&self) -> Result<Vec<String>> {
        self.store.emojis().await
    }

    /// Counts an emoji picked, and gives the emojis, the most used first.
    pub async fn use_emoji(&self, emoji: &str) -> Result<Vec<String>> {
        self.store.use_emoji(emoji, now()).await
    }

    /// Keeps the interface's preferences, and tells every client.
    pub async fn set_preferences(&self, preferences: Preferences) -> Result<()> {
        self.store.set_preferences(&preferences).await?;
        let _ = self.changes.send(Change::Preferences(preferences));
        Ok(())
    }

    /// Resolves once [`Self::stop`] has been called, for whoever follows the
    /// changes to stop following them.
    pub async fn stopped(&self) {
        let mut stopped = self.stopped.subscribe();
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RadioId, Running>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        let _ = self.changes.send(Change::Radios);
    }

    /// Tells every client of a change.
    pub(crate) fn tell(&self, change: Change) {
        let _ = self.changes.send(change);
    }

    fn radio(&self, record: RadioRecord) -> Radio {
        Radio {
            id: record.id,
            link: self.link(record.id),
            name: record.settings.name,
            kind: record.settings.kind,
            address: record.settings.address,
            enabled: record.settings.enabled,
            shown: record.settings.shown,
            short_name: (record.settings.kind == RadioKind::Meshtastic)
                .then(|| self.short_name(record.id))
                .flatten(),
            badge: record.settings.badge,
            last_connected: record.last_connected,
            map_max_age: record.map_max_age,
            map_waypoints: record.map_waypoints,
            // Read from the store where it is asked for.
            unread: 0,
        }
    }

    /// The short name of a radio's own node, as it last said it.
    fn short_name(&self, id: RadioId) -> Option<String> {
        let lock = self.lock();
        let running = lock.get(&id)?;
        running.mesh.read(|state| {
            let local = state.local.as_ref()?.num;
            let short = &state.nodes.get(&local)?.user.as_ref()?.short_name;
            (!short.trim().is_empty()).then(|| short.clone())
        })
    }

    /// What was stored of a radio, to start its mesh from.
    async fn stored_state(&self, id: RadioId) -> Result<MeshState> {
        let local = self.store.local(id).await?;
        let nodes = self.store.nodes(id).await?;
        Ok(MeshState {
            local,
            nodes: nodes.into_iter().map(|node| (node.num, node)).collect(),
            ..MeshState::default()
        })
    }

    /// Starts a radio's mesh from `state`, and connects to it if it is
    /// enabled and can be.
    fn run(&self, record: &RadioRecord, state: MeshState) {
        let id = record.id;
        let mesh = Mesh::new(MeshState {
            link: mesh::LinkState::Disconnected,
            ..state
        });
        let mut tasks = vec![self.forward(id, &mesh), self.persist(id, &mesh)];
        let mut enabled = record.settings.enabled;
        if enabled {
            match validate(&record.settings) {
                Ok(transport) => {
                    tracing::info!(radio = id, name = %record.settings.name, %transport, "connecting");
                    tasks.push(mesh.spawn(protocol_of(record.settings.kind), transport));
                }
                Err(error) => {
                    tracing::warn!(radio = id, ?error, "cannot connect to this radio");
                    enabled = false;
                }
            }
        }
        self.lock().insert(
            id,
            Running {
                mesh,
                protocol: protocol_of(record.settings.kind),
                enabled,
                tasks,
            },
        );
    }

    /// Passes a radio's events on, saying whose they are.
    fn forward(&self, id: RadioId, mesh: &Mesh) -> JoinHandle<()> {
        let mut events = mesh.subscribe();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            loop {
                let change = match events.recv().await {
                    // These go out once kept, from `persist`.
                    Ok(
                        mesh::Event::Message(_)
                        | mesh::Event::Waypoint(_)
                        | mesh::Event::Packet(_)
                        | mesh::Event::Ack(_)
                        | mesh::Event::Traceroute(_)
                        | mesh::Event::AdminAnswer { .. },
                    ) => continue,
                    Ok(event) => Change::Mesh(id, event),
                    Err(RecvError::Lagged(_)) => Change::Lagged,
                    Err(RecvError::Closed) => return,
                };
                let _ = changes.send(change);
            }
        })
    }

    /// Stores what a radio learns as it learns it: each node as it changes,
    /// everything at once when the radio syncs, and when it was last connected
    /// as it drops.
    fn persist(&self, id: RadioId, mesh: &Mesh) -> JoinHandle<()> {
        let mut events = mesh.subscribe();
        let mesh = mesh.clone();
        let store = self.store.clone();
        let changes = self.changes.clone();
        let logins = Arc::clone(&self.logins);
        tokio::spawn(async move {
            let mut connected = false;
            let mut expiry = tokio::time::interval(EXPIRY_TICK);
            loop {
                let event = tokio::select! {
                    event = events.recv() => event,
                    _ = expiry.tick() => {
                        if let Err(err) = expire(&store, &changes, id).await {
                            tracing::warn!(
                                radio = id,
                                error = format_args!("{err:#}"),
                                "cannot store"
                            );
                        }
                        continue;
                    }
                };
                let stored = match event {
                    Ok(mesh::Event::Node(node)) => store.save_node(id, &node).await,
                    Ok(mesh::Event::NodeRemoved(num)) => store.remove_node(id, num).await,
                    Ok(mesh::Event::Packet(heard)) => {
                        keep_packet(&store, &changes, id, &heard).await
                    }
                    Ok(mesh::Event::Traceroute(answer)) => {
                        answer_traceroute(&store, &changes, id, &mesh, &answer).await
                    }
                    Ok(mesh::Event::Waypoint(heard)) => {
                        crate::waypoints::keep_heard(&store, &changes, id, &heard).await
                    }
                    Ok(mesh::Event::Message(heard))
                        if heard.position.is_some()
                            && !answers_ours(&store, id, heard.request_id).await =>
                    {
                        Ok(())
                    }
                    Ok(mesh::Event::Message(heard)) => {
                        let local = mesh.read(|s| s.local.as_ref().map(|l| l.num));
                        // A position answering ours says ours got there.
                        let answered = heard
                            .position
                            .and(heard.request_id)
                            .map(|asked| (asked, heard.from));
                        let mut row = heard_row(heard, local);
                        // MeshCore's reactions come as text.
                        if mesh.read(|s| s.meshcore.is_some())
                            && let Err(err) = read_reaction(&store, id, &mesh, &mut row).await
                        {
                            tracing::warn!(radio = id, error = format_args!("{err:#}"), "reaction");
                        }
                        let kept = store.add_message(id, &row).await;
                        if let Some((asked, from)) = answered {
                            match store.ack(id, asked, from, None).await {
                                Ok(Some((message, status, error))) => {
                                    let _ = changes.send(Change::MessageStatus {
                                        radio: id,
                                        id: message,
                                        status,
                                        error,
                                    });
                                }
                                Ok(None) => {}
                                Err(err) => tracing::warn!(
                                    radio = id,
                                    error = format_args!("{err:#}"),
                                    "cannot store"
                                ),
                            }
                        }
                        kept.map(|kept| {
                            if let Some(message) = kept {
                                let _ = changes.send(Change::Message(id, message));
                            }
                        })
                    }
                    Ok(mesh::Event::Ack(ack)) => {
                        // A relay giving up on a request, a traceroute among
                        // them, or a node with nothing to tell, says so this way.
                        if let Some(error) = &ack.error {
                            match store.fail_request(id, ack.packet_id, error).await {
                                Ok(Some(request)) => {
                                    let _ = changes.send(Change::Request(id, request));
                                }
                                Ok(None) => {}
                                Err(err) => tracing::warn!(
                                    radio = id,
                                    error = format_args!("{err:#}"),
                                    "cannot store"
                                ),
                            }
                        }
                        let answered = store
                            .ack(id, ack.packet_id, ack.from, ack.error.as_deref())
                            .await;
                        answered.map(|answered| {
                            if let Some((message, status, error)) = answered {
                                let _ = changes.send(Change::MessageStatus {
                                    radio: id,
                                    id: message,
                                    status,
                                    error,
                                });
                            }
                        })
                    }
                    Ok(mesh::Event::Reply(reply)) => {
                        keep_reply(&store, &changes, id, &mesh, &reply).await
                    }
                    Ok(mesh::Event::Traced {
                        request_id,
                        from,
                        hops,
                        snrs,
                        last_snr,
                    }) => {
                        let traced = Traced {
                            hops: &hops,
                            snrs: &snrs,
                            last_snr,
                        };
                        answer_meshcore_trace(
                            &store, &changes, id, &mesh, request_id, from, &traced,
                        )
                        .await
                    }
                    // A request given up on, a traceroute among them.
                    Ok(mesh::Event::Unanswered { request_id, from }) => {
                        match store.unanswered_request(id, request_id, from).await {
                            Ok(Some(request)) => {
                                let _ = changes.send(Change::Request(id, request));
                                Ok(())
                            }
                            Ok(None) => Ok(()),
                            Err(err) => Err(err),
                        }
                    }
                    Ok(mesh::Event::RawHeard(heard)) => {
                        keep_raw(&store, &changes, id, &mesh, heard).await
                    }
                    Ok(mesh::Event::Route(route)) => {
                        keep_route(&store, &changes, id, &mesh, &route).await
                    }
                    Ok(mesh::Event::Synced) => {
                        let first = !connected;
                        connected = true;
                        // Never in sync before: new to meshctl, which has
                        // nothing of its own to set on it yet.
                        let new = first
                            && matches!(store.radio(id).await, Ok(Some(r)) if r.last_connected.is_none());
                        let saved = match save_all(&store, id, &mesh).await {
                            Ok(()) => follow_channels(&store, &changes, id, &mesh).await,
                            other => other,
                        };
                        // Its channels silenced as meshctl has them, once a
                        // link: not again as it syncs after taking them. A
                        // new one's, as it has them, taken by meshctl.
                        if first && saved.is_ok() {
                            let (store, mesh, changes) =
                                (store.clone(), mesh.clone(), changes.clone());
                            tokio::spawn(async move {
                                if new {
                                    adopt_mutes(&store, &changes, id, &mesh).await;
                                } else {
                                    push_mutes(&store, id, &mesh).await;
                                }
                            });
                        }
                        saved
                    }
                    // Something was missed: store it all again, if it is current.
                    Err(RecvError::Lagged(_)) if connected => save_all(&store, id, &mesh).await,
                    Ok(mesh::Event::Link(link)) if connected && link != mesh::LinkState::Ready => {
                        connected = false;
                        // Logged in to no room again until it logs in again.
                        logins
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .retain(|(radio, _), _| *radio != id);
                        store.set_last_connected(id, now()).await
                    }
                    Err(RecvError::Closed) => return,
                    _ => Ok(()),
                };
                if let Err(err) = stored {
                    tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
                }
            }
        })
    }

    /// The store, for the tests to put things in it.
    #[cfg(test)]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Puts a mesh in place of a radio's link, for the tests.
    #[cfg(test)]
    pub fn insert_mesh(&self, id: RadioId, mesh: Mesh) {
        let forward = self.forward(id, &mesh);
        self.lock().insert(
            id,
            Running {
                mesh,
                protocol: Protocol::Meshtastic,
                enabled: true,
                tasks: vec![forward],
            },
        );
    }
}

/// Points over a while, at most so many: as they are when they fit, or else
/// averaged in as many stretches of the while, each at its points' mean
/// time. A stretch with none is left out, so that a chart shows the gap.
fn thin(points: Vec<TelemetryPoint>, start: i64, end: i64, most: usize) -> Vec<TelemetryPoint> {
    if points.len() <= most || most == 0 || end <= start {
        return points;
    }
    #[allow(clippy::cast_possible_wrap)]
    let stretch = ((end - start) / most as i64).max(1);
    let mut thinned: Vec<TelemetryPoint> = Vec::with_capacity(most);
    let mut current: Option<(i64, i64, f64, u32)> = None;
    for point in points {
        let index = (point.at - start) / stretch;
        match &mut current {
            Some((at, sum_at, sum, count)) if *at == index => {
                *sum_at += point.at;
                *sum += point.value;
                *count += 1;
            }
            _ => {
                if let Some((_, sum_at, sum, count)) = current.take() {
                    thinned.push(TelemetryPoint {
                        at: sum_at / i64::from(count),
                        value: sum / f64::from(count),
                    });
                }
                current = Some((index, point.at, point.value, 1));
            }
        }
    }
    if let Some((_, sum_at, sum, count)) = current {
        thinned.push(TelemetryPoint {
            at: sum_at / i64::from(count),
            value: sum / f64::from(count),
        });
    }
    thinned
}

/// What a radio refusing a change of settings means for the API.
/// What a MeshCore radio would not do, as it names it, as a setting it
/// cannot have; the rest as any radio's.
fn meshcore_failure(error: AdminError) -> SettingsFailure {
    match error {
        AdminError::Refused(why) => SettingsFailure::Invalid(why),
        AdminError::UnknownNode => SettingsFailure::Invalid("node".into()),
        other => admin_failure(other),
    }
}

/// Whose settings: the radio's own, or another node's, read and changed
/// over the mesh as its admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    Radio,
    Node(u32),
}

/// The parts of a node's settings its pages show at once: its owner and
/// role, its LoRa, and its eight channels.
const NODE_PARTS: [&str; 11] = [
    "owner",
    "config.device",
    "config.lora",
    "channel.0",
    "channel.1",
    "channel.2",
    "channel.3",
    "channel.4",
    "channel.5",
    "channel.6",
    "channel.7",
];

/// The part of the settings with the admin keys.
const SECURITY: &str = "config.security";

/// Whether meshctl has a part of a node's settings, by its name.
fn has_part(settings: &meshctl_core::mesh::Settings, part: &str) -> bool {
    let config = &settings.config;
    match part.split_once('.') {
        Some(("config", name)) => match name {
            "device" => config.device.is_some(),
            "position" => config.position.is_some(),
            "power" => config.power.is_some(),
            "network" => config.network.is_some(),
            "display" => config.display.is_some(),
            "lora" => config.lora.is_some(),
            "bluetooth" => config.bluetooth.is_some(),
            "security" => config.security.is_some(),
            _ => false,
        },
        Some(("module", name)) => settings.modules.contains_key(name),
        Some(("channel", index)) => index
            .parse::<u32>()
            .is_ok_and(|i| settings.channels.contains_key(&i)),
        _ => match part {
            "owner" => settings.owner.is_some(),
            "metadata" => settings.metadata.is_some(),
            _ => false,
        },
    }
}

/// Why a node would not tell its settings: refused as its admin, or not
/// reached.
fn node_admin_failure(error: AdminError) -> SettingsFailure {
    match error {
        AdminError::NotConnected => SettingsFailure::NotConnected,
        AdminError::Unsupported => SettingsFailure::Unsupported,
        AdminError::UnknownNode | AdminError::NoPath => SettingsFailure::UnknownNode,
        AdminError::OwnNode => SettingsFailure::Invalid("the radio's own node".into()),
        AdminError::NoAnswer => SettingsFailure::NoAnswer,
        AdminError::Refused(reason) => match reason.as_str() {
            "ADMIN_PUBLIC_KEY_UNAUTHORIZED" | "NOT_AUTHORIZED" => SettingsFailure::NotAdmin,
            // Sent, tried, and not heard back.
            "MAX_RETRANSMIT" | "NO_RESPONSE" | "TIMEOUT" => SettingsFailure::NoAnswer,
            _ => SettingsFailure::Refused(reason),
        },
    }
}

fn admin_failure(error: AdminError) -> SettingsFailure {
    match error {
        AdminError::NotConnected => SettingsFailure::NotConnected,
        AdminError::NoAnswer => SettingsFailure::NoAnswer,
        AdminError::Unsupported => SettingsFailure::Unsupported,
        // Settings name no node.
        AdminError::UnknownNode | AdminError::OwnNode | AdminError::NoPath => {
            SettingsFailure::NotConnected
        }
        // Settings are not acknowledged, but read back.
        AdminError::Refused(_) => SettingsFailure::NoAnswer,
    }
}

/// A message heard, as kept.
///
/// One from the radio itself was sent through it by another client (a phone,
/// or another one behind the same tasticsplitter): it is ours as much as one
/// sent from meshctl, and as far as anyone here can tell, it went out.
/// Sets a Meshtastic radio's channels silenced as meshctl has them, which
/// is what counts: each that differs, at once. Nothing on MeshCore, whose
/// channels have no such setting.
async fn push_mutes(store: &Store, id: RadioId, mesh: &Mesh) {
    use meshctl_proto::meshtastic::channel::Role;
    if mesh.read(|state| state.meshcore.is_some()) {
        return;
    }
    let channels: Vec<meshctl_proto::meshtastic::Channel> =
        mesh.read(|state| state.settings.channels.values().cloned().collect());
    let mut changed = Vec::new();
    for mut channel in channels {
        if channel.role == Role::Disabled as i32 {
            continue;
        }
        let Ok(slot) = u32::try_from(channel.index) else {
            continue;
        };
        let Ok(muted) = store.muted(id, Peer::Channel { channel: slot }).await else {
            continue;
        };
        let Some(settings) = channel.settings.as_mut() else {
            continue;
        };
        let modules = settings
            .module_settings
            .get_or_insert_with(Default::default);
        if modules.is_muted != muted {
            modules.is_muted = muted;
            changed.push(channel);
        }
    }
    if changed.is_empty() {
        return;
    }
    let slots: Vec<i32> = changed.iter().map(|c| c.index).collect();
    let change = meshctl_core::radio::SettingsChange {
        channels: changed,
        ..Default::default()
    };
    match mesh.change_settings(change).await {
        Ok(()) => tracing::info!(radio = id, ?slots, "channels silenced as meshctl has them"),
        Err(error) => tracing::warn!(radio = id, ?error, "channels not silenced on the radio"),
    }
}

/// Takes a radio's channels silenced as it has them, the first time meshctl
/// has it in sync: what it had before meshctl knew it is kept, not undone.
/// From then on, meshctl's are set on it.
async fn adopt_mutes(store: &Store, changes: &broadcast::Sender<Change>, id: RadioId, mesh: &Mesh) {
    use meshctl_proto::meshtastic::channel::Role;
    if mesh.read(|state| state.meshcore.is_some()) {
        return;
    }
    let muted: Vec<u32> = mesh.read(|state| {
        state
            .settings
            .channels
            .values()
            .filter(|c| c.role != Role::Disabled as i32)
            .filter(|c| {
                c.settings
                    .as_ref()
                    .and_then(|s| s.module_settings.as_ref())
                    .is_some_and(|m| m.is_muted)
            })
            .filter_map(|c| u32::try_from(c.index).ok())
            .collect()
    });
    for channel in muted {
        let peer = Peer::Channel { channel };
        match store.set_muted(id, peer, true).await {
            Ok(true) => {
                tracing::info!(radio = id, channel, "channel silenced as the radio has it");
                let _ = changes.send(Change::ConversationMuted {
                    radio: id,
                    peer,
                    muted: true,
                });
            }
            Ok(false) => {}
            Err(err) => tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store"),
        }
    }
}

/// Whether a position said to the radio is one to keep among its messages:
/// sent to it on its own, or answering one the radio sent. Not one that
/// answers the radio asking for it from a node's page, which goes to the
/// node alone.
async fn answers_ours(store: &Store, radio: RadioId, request_id: Option<u32>) -> bool {
    let Some(request_id) = request_id else {
        return true;
    };
    store
        .message_by_packet(radio, request_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|asked| asked.outgoing && asked.position.is_some())
}

/// How precisely a channel of a Meshtastic radio shares positions, in bits,
/// as the firmware has it: 0 for none, and no more than a public key lets.
/// None if there is no such channel, or it is off.
pub(crate) fn shared_precision(settings: &mesh::Settings, slot: u32) -> Option<u32> {
    use meshctl_proto::meshtastic::channel::Role;
    let channel = settings.channels.get(&slot)?;
    if channel.role == Role::Disabled as i32 {
        return None;
    }
    let config = channel.settings.as_ref();
    let precision = config
        .and_then(|c| c.module_settings.as_ref())
        .map_or(0, |m| m.position_precision);
    let primary_psk = settings
        .channels
        .get(&0)
        .and_then(|c| c.settings.as_ref())
        .map_or(&[][..], |s| s.psk.as_slice());
    let public = crate::channels::public_key(
        config.map_or(&[][..], |c| c.psk.as_slice()),
        channel.role == Role::Secondary as i32,
        primary_psk,
    );
    Some(if public {
        precision.min(meshctl_api::PUBLIC_POSITION_PRECISION)
    } else {
        precision
    })
}

fn heard_row(heard: mesh::TextMessage, local: Option<u32>) -> NewMessageRow {
    let ours = local == Some(heard.from);
    NewMessageRow {
        packet_id: heard.packet_id,
        from: heard.from,
        sender_name: heard.sender_name,
        to: heard.to,
        channel: heard.channel,
        text: heard.text,
        at: i64::from(heard.at),
        outgoing: ours,
        status: if ours {
            MessageStatus::Sent
        } else {
            MessageStatus::Received
        },
        reply_id: heard.reply_id,
        emoji: heard.emoji,
        snr: heard.snr,
        rssi: heard.rssi,
        hops_away: heard.hops_away,
        via_mqtt: heard.via_mqtt,
        scope: None,
        sent_at: heard.sent_at.map(i64::from),
        author: heard.author,
        position: heard.position.as_ref().map(crate::web::convert::position),
    }
}

/// A MeshCore message heard that is a reaction, as its apps send them: kept
/// as a reaction, its emoji its text, to the message it names in the same
/// conversation, the newest that matches; to none if none does.
async fn read_reaction(
    store: &Store,
    radio: RadioId,
    mesh: &Mesh,
    row: &mut NewMessageRow,
) -> Result<()> {
    let channel = row.to == mesh::BROADCAST;
    let Some(reaction) = crate::reactions::parse(&row.text, channel) else {
        return Ok(());
    };
    let peer = if channel {
        Peer::Channel {
            channel: row.channel,
        }
    } else if row.outgoing {
        Peer::Node { node: row.to }
    } else {
        Peer::Node { node: row.from }
    };
    let own = mesh.read(|s| s.meshcore.as_ref().map(|m| m.name.clone()));
    let messages = store.messages(radio, peer, None, REACTION_REACH).await?;
    let target = messages.iter().rev().filter(|m| !m.emoji).find(|m| {
        let Some(at) = m.sent_at.and_then(|at| u32::try_from(at).ok()) else {
            return false;
        };
        let sender = if !channel {
            None
        } else if m.outgoing {
            own.clone()
        } else {
            m.sender_name.clone()
        };
        reaction.names(&m.text, sender.as_deref(), at)
    });
    row.reply_id = target.map(|m| m.packet_id);
    row.emoji = true;
    row.text = reaction.emoji().to_owned();
    Ok(())
}

/// How far back in a conversation a reaction's message is looked for.
const REACTION_REACH: u32 = 300;

/// Takes the channels a radio was just read with: each slot's conversation
/// is its channel's. If any slot has another, the radio is said to be in
/// sync again, for whoever shows it to read it anew.
async fn follow_channels(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    id: RadioId,
    mesh: &Mesh,
) -> Result<()> {
    let now = mesh.read(crate::channels::slots_of);
    // A radio always has its primary: none read is nothing read, not every
    // channel gone.
    if now.is_empty() {
        return Ok(());
    }
    if store.follow_channels(id, &now, crate::store::now()).await? {
        tracing::info!(radio = id, "channels moved");
        let _ = changes.send(Change::Mesh(id, mesh::Event::Synced));
    }
    Ok(())
}

async fn save_all(store: &Store, id: RadioId, mesh: &Mesh) -> Result<()> {
    let (local, nodes) = mesh.read(|state| {
        (
            state.local.clone(),
            state.nodes.values().cloned().collect::<Vec<_>>(),
        )
    });
    store.save_sync(id, local.as_ref(), &nodes, now()).await
}

/// A Meshtastic node to share, as its apps read it: the contact, in
/// base64url without padding, after `https://meshtastic.org/v/#`.
fn meshtastic_contact(num: u32, user: meshctl_proto::meshtastic::User) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use prost::Message;
    let contact = meshctl_proto::meshtastic::SharedContact {
        node_num: num,
        user: Some(user),
        should_ignore: false,
        manually_verified: false,
    };
    format!(
        "https://meshtastic.org/v/#{}",
        URL_SAFE_NO_PAD.encode(contact.encode_to_vec())
    )
}

/// A node's user as Meshtastic's protobufs have it, from what its node info
/// said: what a contact carries.
fn meshtastic_user(num: u32, user: &meshctl_core::mesh::User) -> meshctl_proto::meshtastic::User {
    use meshctl_proto::meshtastic::{HardwareModel, config::device_config::Role};
    meshctl_proto::meshtastic::User {
        id: format!("!{num:08x}"),
        long_name: user.long_name.clone(),
        short_name: user.short_name.clone(),
        hw_model: HardwareModel::from_str_name(&user.hw_model).map_or(0, |m| m as i32),
        role: Role::from_str_name(&user.role).map_or(0, |r| r as i32),
        public_key: user.public_key.clone(),
        is_licensed: user.licensed,
        is_unmessagable: Some(user.unmessagable),
        ..Default::default()
    }
}

/// A MeshCore node to share, as its apps read it: its name, its key in hex
/// and its kind (1 a companion, 2 a repeater, 3 a room, 4 a sensor), as
/// `meshcore://contact/add` asks. None without a whole key.
fn meshcore_contact(name: &str, key: &[u8], role: &str) -> Option<String> {
    if key.len() != 32 {
        return None;
    }
    let kind = match role {
        "REPEATER" => 2,
        "ROOM" => 3,
        "SENSOR" => 4,
        _ => 1,
    };
    let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let name: String = form_urlencoded::byte_serialize(name.as_bytes()).collect();
    Some(format!(
        "meshcore://contact/add?name={name}&public_key={key}&type={kind}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meshtastic_contact_reads_back() {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use prost::Message;
        let user = meshctl_proto::meshtastic::User {
            id: "!1234abcd".into(),
            long_name: "Casa".into(),
            short_name: "CASA".into(),
            public_key: vec![7; 32],
            ..Default::default()
        };
        let url = meshtastic_contact(0x1234_abcd, user.clone());
        let encoded = url.strip_prefix("https://meshtastic.org/v/#").unwrap();
        assert!(!encoded.contains('=') && !encoded.contains('+') && !encoded.contains('/'));
        let back = meshctl_proto::meshtastic::SharedContact::decode(
            URL_SAFE_NO_PAD.decode(encoded).unwrap().as_slice(),
        )
        .unwrap();
        assert_eq!(back.node_num, 0x1234_abcd);
        assert_eq!(back.user, Some(user));
    }

    #[test]
    fn a_heard_node_shares_what_its_node_info_said() {
        use meshctl_proto::meshtastic::{HardwareModel, config::device_config::Role};
        let heard = meshctl_core::mesh::User {
            long_name: "Repetidor".into(),
            short_name: "REP".into(),
            hw_model: "HELTEC_V3".into(),
            role: "ROUTER".into(),
            public_key: vec![9; 32],
            licensed: false,
            unmessagable: true,
        };
        let user = meshtastic_user(0x0bad_cafe, &heard);
        assert_eq!(user.id, "!0badcafe");
        assert_eq!(user.hw_model, HardwareModel::HeltecV3 as i32);
        assert_eq!(user.role, Role::Router as i32);
        assert_eq!(user.public_key, vec![9; 32]);
        assert_eq!(user.is_unmessagable, Some(true));
    }

    #[test]
    fn a_meshcore_contact_says_its_name_key_and_kind() {
        let key: Vec<u8> = (0..32).collect();
        assert_eq!(
            meshcore_contact("Mi radio", &key, "REPEATER").unwrap(),
            concat!(
                "meshcore://contact/add?name=Mi+radio&public_key=",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f&type=2"
            )
        );
        assert_eq!(meshcore_contact("X", &key[..8], "CHAT"), None);
    }

    fn settings(name: &str, kind: RadioKind, address: &str) -> RadioSettings {
        RadioSettings {
            name: name.into(),
            kind,
            address: address.into(),
            enabled: true,
            shown: true,
            badge: Default::default(),
        }
    }

    #[test]
    fn an_answer_reads_as_two_routes_from_end_to_end() {
        let answer = mesh::TracerouteAnswer {
            request_id: 1,
            from: 9,
            route: vec![5, mesh::UNKNOWN_HOP],
            snr_towards: vec![Some(6.0), None, Some(2.5)],
            route_back: vec![5],
            snr_back: vec![Some(-1.5), Some(10.0)],
        };
        let (towards, back) = routes(7, &answer);
        let hop = Hop::known;
        assert_eq!(
            towards,
            [
                hop(Some(7), None),
                hop(Some(5), Some(6.0)),
                hop(None, None),
                hop(Some(9), Some(2.5)),
            ]
        );
        assert_eq!(
            back,
            [
                hop(Some(9), None),
                hop(Some(5), Some(-1.5)),
                hop(Some(7), Some(10.0))
            ]
        );
    }

    #[test]
    fn a_direct_answer_is_one_hop_each_way() {
        let answer = mesh::TracerouteAnswer {
            request_id: 1,
            from: 9,
            route: vec![],
            snr_towards: vec![Some(8.0)],
            route_back: vec![],
            snr_back: vec![Some(7.0)],
        };
        let (towards, back) = routes(7, &answer);
        assert_eq!(towards.len(), 2);
        assert_eq!(back[1].snr, Some(7.0));
    }

    #[test]
    fn what_another_client_sent_through_the_radio_is_ours() {
        let heard = mesh::TextMessage {
            packet_id: 1,
            from: 7,
            to: 9,
            channel: 0,
            text: "hola".into(),
            at: 1,
            reply_id: None,
            sender_name: None,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            sent_at: None,
            author: None,
            position: None,
            request_id: None,
        };
        let ours = heard_row(heard.clone(), Some(7));
        assert!(ours.outgoing);
        assert_eq!(ours.status, MessageStatus::Sent);
        let theirs = heard_row(heard, Some(9));
        assert!(!theirs.outgoing);
        assert_eq!(theirs.status, MessageStatus::Received);
    }

    #[test]
    fn what_a_radio_may_be() {
        let ok = settings("Home", RadioKind::Meshtastic, "192.168.1.10");
        assert!(validate(&ok).is_ok());
        // Each kind takes its own port when none is given.
        let port = |kind| match validate(&settings("Home", kind, "radio.lan")) {
            Ok(Transport::Tcp { port, .. }) => port,
            other => panic!("{other:?}"),
        };
        assert_eq!(port(RadioKind::Meshtastic), 4403);
        assert_eq!(port(RadioKind::Meshcore), 5000);
        let cases = [
            (
                settings("  ", RadioKind::Meshtastic, "h"),
                ErrorCode::InvalidName,
            ),
            (
                settings(&"x".repeat(65), RadioKind::Meshtastic, "h"),
                ErrorCode::InvalidName,
            ),
            (
                settings("Home", RadioKind::Meshtastic, "h:port"),
                ErrorCode::InvalidAddress,
            ),
        ];
        for (settings, error) in cases {
            assert_eq!(validate(&settings).unwrap_err(), error, "{settings:?}");
        }
    }

    #[test]
    fn what_a_radio_button_may_show() {
        let with = |color: Option<&str>, content| {
            let mut radio = settings("Home", RadioKind::Meshtastic, "192.168.1.10");
            radio.badge = Badge {
                color: color.map(Into::into),
                content,
            };
            radio
        };
        let text = |text: &str| BadgeContent::Text { text: text.into() };
        let emoji = |emoji: &str| BadgeContent::Emoji {
            emoji: emoji.into(),
        };
        let good = [
            with(Some("#F0a63a"), BadgeContent::Initials),
            with(Some("teal"), BadgeContent::Initials),
            with(None, text("Ñ")),
            with(None, text(" TM ")),
            with(None, text("G3L0")),
            with(None, text("ÑU")),
            with(None, text("🦊")),
            with(None, emoji("📡")),
            with(None, emoji("👨‍👩‍👧")),
            with(None, emoji("🇪🇸")),
            with(
                Some("#000000"),
                BadgeContent::Icon {
                    icon: meshctl_api::BadgeIcon::Meshcore,
                },
            ),
        ];
        for radio in good {
            assert!(validate(&radio).is_ok(), "{radio:?}");
        }
        let bad = [
            with(Some("f0a63a"), BadgeContent::Initials),
            with(Some("red"), BadgeContent::Initials),
            with(Some("#f0a63"), BadgeContent::Initials),
            with(Some("#g0a63a"), BadgeContent::Initials),
            with(Some("#f0a63a00"), BadgeContent::Initials),
            with(None, text("")),
            with(None, text("ABCDE")),
            with(None, text("ÑAND")),
            with(None, text("🦊M")),
            with(None, text("A B")),
            with(None, emoji("")),
            with(None, emoji("ok")),
            with(None, emoji("📡 📡")),
            with(None, emoji(&"📡".repeat(17))),
        ];
        for radio in bad {
            assert_eq!(
                validate(&radio).unwrap_err(),
                ErrorCode::InvalidBadge,
                "{radio:?}"
            );
        }
        // Kept trimmed, and the colour in lower case.
        let kept = tidy(with(Some("#F0A63A"), text(" TM ")));
        assert_eq!(kept.badge.color.as_deref(), Some("#f0a63a"));
        assert_eq!(kept.badge.content, text("TM"));
    }

    #[tokio::test]
    async fn radios_keep_the_order_given() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let mut ids = Vec::new();
        for name in ["A", "B", "C"] {
            let mut radio = settings(name, RadioKind::Meshtastic, "127.0.0.1:1");
            radio.enabled = false;
            ids.push(radios.add(radio).await.unwrap().id);
        }
        let names = async || {
            let list = radios.list().await.unwrap();
            list.into_iter().map(|r| r.name).collect::<Vec<_>>()
        };
        assert_eq!(names().await, ["A", "B", "C"]);

        let mut changes = radios.subscribe();
        assert!(radios.reorder(&[ids[2], ids[0], ids[1]]).await.unwrap());
        assert!(matches!(changes.recv().await.unwrap(), Change::Radios));
        assert_eq!(names().await, ["C", "A", "B"]);

        // Every radio, once: else nothing changes.
        for wrong in [
            vec![ids[0], ids[1]],
            vec![ids[0], ids[1], ids[1]],
            vec![ids[0], ids[1], ids[2], 99],
        ] {
            assert!(!radios.reorder(&wrong).await.unwrap(), "{wrong:?}");
        }
        assert_eq!(names().await, ["C", "A", "B"]);

        // One added later goes last.
        let mut last = settings("D", RadioKind::Meshtastic, "127.0.0.1:1");
        last.enabled = false;
        radios.add(last).await.unwrap();
        assert_eq!(names().await, ["C", "A", "B", "D"]);
    }

    #[tokio::test]
    async fn a_new_look_keeps_the_link() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let radio = radios
            .add(settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        let running = |radios: &Radios| {
            let lock = radios.lock();
            let tasks = &lock[&radio.id].tasks;
            tasks.iter().map(|task| task.id()).collect::<Vec<_>>()
        };
        let before = running(&radios);

        let mut look = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        look.shown = false;
        look.badge.color = Some("#3a86f0".into());
        let changed = radios.update(radio.id, look).await.unwrap().unwrap();
        assert!(!changed.shown);
        assert_eq!(changed.badge.color.as_deref(), Some("#3a86f0"));
        assert_eq!(running(&radios), before);

        // Another address is another link.
        let moved = settings("Home", RadioKind::Meshtastic, "127.0.0.1:2");
        radios.update(radio.id, moved).await.unwrap();
        assert_ne!(running(&radios), before);
    }

    #[tokio::test]
    async fn a_disabled_radio_keeps_its_mesh_but_no_link() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let mut changes = radios.subscribe();

        // Nothing listens there: the link keeps trying, which is all we need.
        let radio = radios
            .add(settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        assert!(matches!(changes.recv().await.unwrap(), Change::Radios));
        assert_ne!(radios.link(radio.id), LinkState::Disabled);

        let mut off = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        off.enabled = false;
        let radio = radios.update(radio.id, off).await.unwrap().unwrap();
        assert_eq!(radio.link, LinkState::Disabled);
        assert!(radios.mesh(radio.id).is_some());

        assert!(radios.remove(radio.id).await.unwrap());
        assert!(radios.mesh(radio.id).is_none());
        assert!(radios.list().await.unwrap().is_empty());
    }

    #[test]
    fn points_are_thinned_by_stretches_of_the_while() {
        let point = |at, value| TelemetryPoint { at, value };
        let few = vec![point(10, 1.0), point(20, 2.0)];
        assert_eq!(thin(few.clone(), 0, 100, 5), few);
        // Ten points over 100 s in five stretches of 20 s; nothing from 60 to
        // 80, a gap that stays one.
        let many: Vec<_> = [0, 5, 20, 25, 30, 35, 50, 55, 85, 95]
            .into_iter()
            .map(|at| point(at, f64::from(at as u32)))
            .collect();
        assert_eq!(
            thin(many, 0, 100, 5),
            [
                point(2, 2.5),
                point(27, 27.5),
                point(52, 52.5),
                point(90, 90.0)
            ]
        );
    }

    #[tokio::test]
    async fn a_node_is_checked_for_its_key_and_hardware() {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use meshctl_proto::meshtastic::{HardwareModel, PortNum, User};
        use prost::Message;

        let store = Store::in_memory().await;
        let mut off = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        off.enabled = false;
        let radio = store.add_radio(&off).await.unwrap().id;
        let shared = vec![5; 32];
        let heard = u32::try_from(now()).unwrap();
        let week_ago = heard - 8 * 24 * 3600;
        let mut state = MeshState::default();
        for (num, key, last_heard) in [
            (1, &shared, heard),
            (2, &shared, heard - 60),
            // Heard too long ago to count.
            (3, &shared, week_ago),
            (4, &vec![9; 32], heard),
        ] {
            let mut node = mesh::Node {
                num,
                user: Some(mesh::User {
                    public_key: key.clone(),
                    ..Default::default()
                }),
                position: None,
                last_heard: Some(last_heard),
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
            };
            node.user.as_mut().unwrap().long_name = format!("N{num}");
            state.nodes.insert(num, node.clone());
            // One more, its node info not heard yet.
            if num == 4 {
                node.num = 5;
                node.user = None;
                state.nodes.insert(5, node);
            }
        }
        // Node 1's node infos: two keys made apart, a copy of one damaged
        // on the air, and two hardware models.
        let mut damaged = shared.clone();
        damaged[0] ^= 0b101;
        for (id, key, model) in [
            (9, &shared, HardwareModel::HeltecV3),
            (10, &shared, HardwareModel::HeltecV3),
            (11, &damaged, HardwareModel::HeltecV3),
            (12, &vec![0xaa; 32], HardwareModel::Rak4631),
        ] {
            let user = User {
                public_key: key.clone(),
                hw_model: model as i32,
                ..Default::default()
            };
            let heard_packet = mesh::Heard {
                packet_id: id,
                from: 1,
                to: mesh::BROADCAST,
                channel: 0,
                portnum: Some(PortNum::NodeinfoApp as i32),
                payload: user.encode_to_vec(),
                request_id: None,
                reply_id: None,
                want_response: false,
                pki: false,
                at: id,
                snr: None,
                rssi: None,
                hop_limit: 3,
                hop_start: 3,
                relay_node: None,
                via_mqtt: false,
            };
            store.add_packet(radio, &heard_packet).await.unwrap();
        }
        let radios = Radios::start(store).await.unwrap();
        radios.insert_mesh(radio, Mesh::new(state));

        let checks = radios.checks(radio, 1).await.unwrap().unwrap();
        assert_eq!(
            checks,
            vec![
                NodeCheck::PublicKeyShared { nodes: vec![2] },
                NodeCheck::PublicKeyConflict {
                    keys: vec![BASE64.encode([0xaa; 32]), BASE64.encode(&shared)],
                },
                NodeCheck::HardwareChanged {
                    models: vec!["RAK4631".into(), "HELTEC_V3".into()],
                },
            ]
        );
        // Nothing amiss with the node of its own key, nor with one whose node
        // info was not heard yet; none for one unknown.
        assert_eq!(radios.checks(radio, 4).await.unwrap(), Some(Vec::new()));
        assert_eq!(radios.checks(radio, 5).await.unwrap(), Some(Vec::new()));
        assert_eq!(radios.checks(radio, 99).await.unwrap(), None);
    }

    #[tokio::test]
    async fn stored_radios_start_with_what_they_knew() {
        let store = Store::in_memory().await;
        let mut off = settings("Car", RadioKind::Meshtastic, "/dev/ttyACM0");
        off.enabled = false;
        let added = store.add_radio(&off).await.unwrap();
        let faro = mesh::Node {
            num: 9,
            user: None,
            position: None,
            last_heard: Some(5),
            snr: None,
            rssi: None,
            hops_away: Some(1),
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
            channel_utilization: None,
            on_radio: None,
            status: None,
            status_at: None,
        };
        store.save_node(added.id, &faro).await.unwrap();

        let radios = Radios::start(store).await.unwrap();
        let known = radios.mesh(added.id).unwrap().read(|s| s.nodes.clone());
        assert_eq!(known.get(&9), Some(&faro));
        assert_eq!(radios.link(added.id), LinkState::Disabled);

        radios.stop().await;
        assert!(radios.mesh(added.id).is_none());
    }

    #[tokio::test]
    async fn a_meshtastic_radio_goes_by_its_short_name() {
        let store = Store::in_memory().await;
        let mut off = settings("Car", RadioKind::Meshtastic, "/dev/ttyACM0");
        off.enabled = false;
        let car = store.add_radio(&off).await.unwrap();
        let mut core = settings("Core", RadioKind::Meshcore, "/dev/ttyACM1");
        core.enabled = false;
        let core = store.add_radio(&core).await.unwrap();
        let own = |num| mesh::Node {
            num,
            user: Some(mesh::User {
                long_name: "G3L0 M0V1L".into(),
                short_name: "G3L0".into(),
                hw_model: "HELTEC_V3".into(),
                role: "CLIENT".into(),
                ..Default::default()
            }),
            position: None,
            last_heard: None,
            snr: None,
            rssi: None,
            hops_away: Some(0),
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
            channel_utilization: None,
            on_radio: None,
            status: None,
            status_at: None,
        };
        let local = |num| mesh::LocalNode {
            num,
            firmware_version: None,
            hw_model: None,
            role: None,
            region: None,
            modem_preset: None,
        };
        for (radio, num) in [(car.id, 7), (core.id, 8)] {
            store
                .save_sync(radio, Some(&local(num)), &[own(num)], 1)
                .await
                .unwrap();
        }

        let radios = Radios::start(store).await.unwrap();
        let short = |id| radios.get(id);
        assert_eq!(
            short(car.id).await.unwrap().unwrap().short_name.as_deref(),
            Some("G3L0")
        );
        // MeshCore's nodes have no short names.
        assert_eq!(short(core.id).await.unwrap().unwrap().short_name, None);
        radios.stop().await;
    }
}
