// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The MeshCore driver: keeps a companion radio connected and its mesh in the
//! same [`MeshState`] the Meshtastic one keeps, so that nothing above this
//! module has to know which of the two a radio speaks.
//!
//! Where MeshCore differs, it is made to fit here:
//!
//! - Nodes are the **contacts** the radio keeps, each known by a public key.
//!   The first four bytes of the key stand for the node number everything
//!   else uses; the whole key stays here, for sending to it.
//! - A **channel message** carries no sender. Clients write their name into
//!   the text, `NAME: message`, and that is where it is read back from.
//! - Messages are **pulled**: the radio announces it has some, and is asked
//!   for them one by one. Those that arrived while nobody was connected were
//!   announced to nobody, and are drained on connecting.
//! - A **direct message** is acknowledged with a tag the radio gives when it
//!   takes the message; a channel message is not acknowledged at all.
//!
//! The protocol itself is in [`protocol`]; what this module knows of how it
//! behaves it learned from loratrix and coresplitter, which have been running
//! against real radios.

pub mod protocol;

use crate::link::{self, Framing, Link, Transport};
use crate::mesh::{
    Ack, BROADCAST, Channel, Event, LinkState, LocalNode, MeshState, Node, Position, TextMessage,
    User,
};
use crate::radio::{
    AdminError, Mesh, OutgoingText, RECONNECT_MAX, RECONNECT_MIN, REREAD_INTERVAL, STABLE_SESSION,
    SendError, Sent, now,
};
use protocol::{
    ChannelMessage, Contact, ContactMessage, DeviceInfo, Response, SelfInfo, Stats, command,
};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// The port a MeshCore companion radio serves on over the network.
pub const DEFAULT_TCP_PORT: u16 = 5000;

/// What the radio is told it is talking to: the firmware prints it, as the
/// app that connected.
const APP_NAME: &str = "meshctl";

/// How many channel slots to look at when the radio does not say.
const CHANNELS_DEFAULT: u8 = 8;

/// How long the radio has to answer a command.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the radio has to send every contact it keeps: hundreds of them,
/// one frame each.
const CONTACTS_TIMEOUT: Duration = Duration::from_secs(60);

/// How long after a contact is heard of to read the contacts again: adverts
/// come in bursts, and each read returns every contact that changed.
const CONTACTS_SETTLE: Duration = Duration::from_secs(2);

/// What the radio's path length says for a path it does not know.
const NO_PATH: u8 = 0xff;

/// How far behind its clock may be before it is set: a little drift is
/// not worth writing to the radio for.
const CLOCK_SLACK: u32 = 60;

/// A radio's own settings, as it says them, and as they are changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// What it advertises itself as.
    pub name: String,
    /// Where it says it is, in millionths of a degree; both zero for
    /// nowhere.
    pub lat: i32,
    pub lon: i32,
    /// In kHz.
    pub frequency: u32,
    /// In Hz.
    pub bandwidth: u32,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    /// In dBm.
    pub tx_power: i8,
    /// The most its board allows, in dBm; not changed.
    pub max_tx_power: u8,
    /// Whether it repeats as well; none on firmware without it.
    pub repeat: Option<bool>,
    /// Whether contacts are added only by hand.
    pub manual_add: bool,
    /// Who it shares telemetry with, two bits each: base, location (<< 2)
    /// and environment (<< 4); 0 nobody, 1 those flagged, 2 anyone.
    pub telemetry: u8,
    /// 0 its adverts say nothing of where it is, 1 they say.
    pub location_policy: u8,
    /// Extra acknowledgements it sends.
    pub multi_acks: u8,
    /// How many bytes each hop's hash takes in what it sends, less one;
    /// none on firmware without it.
    pub path_hash_mode: Option<u8>,
    /// Its receive delay base and airtime factor, in thousandths.
    pub tuning: Option<(u32, u32)>,
    /// Which kinds of contact it adds by itself while contacts are added by
    /// hand, and whether it overwrites the oldest, as bits; and up to how far.
    pub autoadd: Option<(u8, u8)>,
    /// The frequency ranges it repeats on, in kHz; not changed.
    pub repeat_frequencies: Vec<(u32, u32)>,
    /// The region what it floods is scoped to, name and key, or none; none
    /// at all on firmware without it.
    pub default_scope: Option<Option<(String, [u8; 16])>>,
}

/// The frequencies a client repeats on, in kHz, where the radio does not say.
const REPEAT_FREQUENCIES: [(u32, u32); 3] =
    [(433_000, 433_000), (869_495, 869_495), (918_000, 918_000)];

impl Settings {
    fn from(me: &SelfInfo, device: Option<&DeviceInfo>, extras: Extras) -> Self {
        Self {
            name: me.name.clone(),
            lat: me.lat,
            lon: me.lon,
            frequency: me.frequency,
            bandwidth: me.bandwidth,
            spreading_factor: me.spreading_factor,
            coding_rate: me.coding_rate,
            tx_power: me.tx_power,
            max_tx_power: me.max_tx_power,
            repeat: device.and_then(|d| d.repeat),
            manual_add: me.manual_add,
            telemetry: me.telemetry,
            location_policy: me.location_policy,
            multi_acks: me.multi_acks,
            path_hash_mode: device.and_then(|d| d.path_hash_mode),
            tuning: extras.tuning,
            autoadd: extras.autoadd,
            repeat_frequencies: extras
                .repeat_frequencies
                .unwrap_or_else(|| REPEAT_FREQUENCIES.to_vec()),
            default_scope: extras.default_scope,
        }
    }
}

/// The settings a radio answers each on its own, as it says them; none of
/// those its firmware does not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras {
    /// Its receive delay base and airtime factor, in thousandths.
    pub tuning: Option<(u32, u32)>,
    /// Which kinds of contact it adds by itself, as bits, and up to how far.
    pub autoadd: Option<(u8, u8)>,
    /// The frequency ranges it repeats on, in kHz.
    pub repeat_frequencies: Option<Vec<(u32, u32)>>,
    /// The region what it floods is scoped to, name and key, or none.
    pub default_scope: Option<Option<(String, [u8; 16])>>,
}

/// A MeshCore radio's channels: how many slots it has, and each set one's
/// 128-bit secret, by its slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Channels {
    pub slots: u8,
    pub secrets: std::collections::BTreeMap<u32, [u8; 16]>,
}

/// The secret of the channel every MeshCore radio comes with, `Public`.
pub const PUBLIC_SECRET: [u8; 16] = [
    0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72,
];

/// A radio's figures, asked for: each as its firmware gives it, none of a
/// kind it does not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub battery_mv: Option<u16>,
    pub storage_used_kb: Option<u32>,
    pub storage_total_kb: Option<u32>,
    pub core: Option<Stats>,
    pub radio: Option<Stats>,
    pub packets: Option<Stats>,
}

/// What an error code says, as the firmware names it.
fn error_name(code: u8) -> &'static str {
    match code {
        1 => "UNSUPPORTED_CMD",
        2 => "NOT_FOUND",
        3 => "TABLE_FULL",
        4 => "BAD_STATE",
        5 => "FILE_IO_ERROR",
        6 => "ILLEGAL_ARG",
        _ => "UNKNOWN",
    }
}

/// The radio as the other nodes see it: where it listens, in words.
fn describe(local: &mut LocalNode, settings: &Settings) {
    // Frequency in kHz, bandwidth in Hz: as people write them.
    local.region = Some(format!("{:.3} MHz", f64::from(settings.frequency) / 1000.0));
    local.modem_preset = Some(format!(
        "BW {} kHz · SF{} · CR{}",
        f64::from(settings.bandwidth) / 1000.0,
        settings.spreading_factor,
        settings.coding_rate
    ));
}

/// A connection to a companion radio, which answers one command at a time:
/// what it says in answer goes to whoever asked, what it pushes on its own
/// goes to `pushes`.
pub struct Client {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    answers: tokio::sync::Mutex<mpsc::UnboundedReceiver<Response>>,
    /// Holds the connection: dropping the client closes it.
    _link: Link,
    _reader: ReaderTask,
}

/// The task that reads the radio, stopped with the client.
struct ReaderTask(JoinHandle<()>);

impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Client {
    /// Connects, and returns the client with where the radio's pushes arrive.
    /// That closes when the link drops.
    pub async fn connect(
        transport: &Transport,
    ) -> io::Result<(Self, mpsc::UnboundedReceiver<Response>)> {
        let mut link = link::connect_framed(transport, Framing::Meshcore).await?;
        let (_, nothing) = mpsc::unbounded_channel();
        let mut frames = std::mem::replace(&mut link.rx, nothing);
        let (answer_tx, answers) = mpsc::unbounded_channel();
        let (push_tx, pushes) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                let Some(response) = Response::parse(&frame) else {
                    tracing::debug!(code = frame.first(), "unreadable frame from the radio");
                    continue;
                };
                let to = if response.is_push() {
                    &push_tx
                } else {
                    &answer_tx
                };
                if to.send(response).is_err() {
                    return;
                }
            }
        });
        Ok((
            Self {
                tx: link.tx.clone(),
                answers: tokio::sync::Mutex::new(answers),
                _link: link,
                _reader: ReaderTask(reader),
            },
            pushes,
        ))
    }

    /// Sends a command and gathers what the radio answers, up to and including
    /// the answer `done` recognises, or an error.
    pub async fn ask(
        &self,
        command: Vec<u8>,
        done: impl Fn(&Response) -> bool,
        timeout: Duration,
    ) -> Result<Vec<Response>, &'static str> {
        let mut answers = self.answers.lock().await;
        // Whatever is left over from a command given up on is not an answer
        // to this one.
        while answers.try_recv().is_ok() {}
        self.tx.send(command).map_err(|_| "the link closed")?;
        let deadline = Instant::now() + timeout;
        let mut got = Vec::new();
        loop {
            let answer = tokio::time::timeout_at(deadline, answers.recv())
                .await
                .map_err(|_| "the radio did not answer")?
                .ok_or("the link closed")?;
            let last = done(&answer) || matches!(answer, Response::Error(_));
            got.push(answer);
            if last {
                return Ok(got);
            }
        }
    }

    /// Sends a command whose answer is a single frame.
    pub async fn ask_one(
        &self,
        command: Vec<u8>,
        done: impl Fn(&Response) -> bool,
    ) -> Result<Response, &'static str> {
        self.ask(command, done, ANSWER_TIMEOUT)
            .await?
            .pop()
            .ok_or("the radio did not answer")
    }

    /// Opens a companion session, saying meshctl is what is connecting.
    pub async fn start(&self) -> Result<SelfInfo, &'static str> {
        match self
            .ask_one(command::app_start(APP_NAME), |r| {
                matches!(r, Response::SelfInfo(_))
            })
            .await?
        {
            Response::SelfInfo(me) => Ok(me),
            _ => Err("the radio refused APP_START"),
        }
    }

    /// Gives a command whose answer is `Ok`: what the radio says otherwise,
    /// as it names it.
    pub async fn done(&self, command: Vec<u8>) -> Result<(), AdminError> {
        match self.ask_one(command, |r| matches!(r, Response::Ok)).await {
            Ok(Response::Ok) => Ok(()),
            Ok(Response::Error(code)) => Err(AdminError::Refused(error_name(code).to_owned())),
            Ok(_) => Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => Err(AdminError::NoAnswer),
            Err(_) => Err(AdminError::NotConnected),
        }
    }

    /// Sets the radio's clock if it is behind, as it keeps no time of its
    /// own across a power cut and dates what it sends by it. It cannot be
    /// set back: a clock ahead is only said.
    pub async fn keep_time(&self) {
        let Ok(Response::CurrentTime(radio)) = self
            .ask_one(command::get_time(), |r| {
                matches!(r, Response::CurrentTime(_))
            })
            .await
        else {
            return;
        };
        let host = now();
        if radio.saturating_add(CLOCK_SLACK) < host {
            match self.done(command::set_time(host)).await {
                Ok(()) => tracing::info!(behind = host - radio, "set the radio's clock"),
                Err(error) => tracing::warn!(?error, "cannot set the radio's clock"),
            }
        } else if radio > host.saturating_add(CLOCK_SLACK) {
            tracing::warn!(
                ahead = radio - host,
                "the radio's clock is ahead, and cannot be set back"
            );
        }
    }

    /// Its battery and storage, and each kind of figure it has.
    pub async fn status(&self) -> Result<Status, AdminError> {
        let mut status = Status::default();
        match self
            .ask_one(command::get_battery(), |r| {
                matches!(r, Response::Battery { .. })
            })
            .await
        {
            Ok(Response::Battery {
                millivolts,
                used_kb,
                total_kb,
            }) => {
                status.battery_mv = Some(millivolts);
                status.storage_used_kb = used_kb;
                status.storage_total_kb = total_kb;
            }
            Ok(_) => {}
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        }
        for kind in 0..3 {
            if let Ok(Response::Stats(stats)) = self
                .ask_one(command::get_stats(kind), |r| {
                    matches!(r, Response::Stats(_))
                })
                .await
            {
                match stats {
                    Stats::Core { .. } => status.core = Some(stats),
                    Stats::Radio { .. } => status.radio = Some(stats),
                    Stats::Packets { .. } => status.packets = Some(stats),
                }
            }
        }
        Ok(status)
    }

    /// The settings it answers each on its own: none of those its firmware
    /// does not have.
    pub async fn extras(&self) -> Extras {
        let mut extras = Extras::default();
        if let Ok(Response::Tuning {
            rx_delay,
            airtime_factor,
        }) = self
            .ask_one(command::get_tuning(), |r| {
                matches!(r, Response::Tuning { .. })
            })
            .await
        {
            extras.tuning = Some((rx_delay, airtime_factor));
        }
        if let Ok(Response::AutoAdd { config, max_hops }) = self
            .ask_one(command::get_autoadd(), |r| {
                matches!(r, Response::AutoAdd { .. })
            })
            .await
        {
            extras.autoadd = Some((config, max_hops));
        }
        if let Ok(Response::RepeatFrequencies(ranges)) = self
            .ask_one(command::get_repeat_frequencies(), |r| {
                matches!(r, Response::RepeatFrequencies(_))
            })
            .await
        {
            extras.repeat_frequencies = Some(ranges);
        }
        if let Ok(Response::DefaultScope(scope)) = self
            .ask_one(command::get_default_scope(), |r| {
                matches!(r, Response::DefaultScope(_))
            })
            .await
        {
            extras.default_scope = Some(scope);
        }
        extras
    }

    pub async fn device(&self) -> Option<DeviceInfo> {
        match self
            .ask_one(command::device_query(), |r| {
                matches!(r, Response::DeviceInfo(_))
            })
            .await
        {
            Ok(Response::DeviceInfo(device)) => Some(device),
            _ => None,
        }
    }

    /// Every contact changed since `since`, and the latest change among them.
    pub async fn contacts(&self, since: u32) -> Result<(Vec<Contact>, u32), &'static str> {
        let answers = self
            .ask(
                command::get_contacts(since),
                |r| matches!(r, Response::ContactEnd(_)),
                CONTACTS_TIMEOUT,
            )
            .await?;
        let mut latest = since;
        let mut contacts = Vec::new();
        for answer in answers {
            match answer {
                Response::Contact(contact) => contacts.push(contact),
                Response::ContactEnd(changed) => latest = latest.max(changed),
                Response::Error(_) => return Err("the radio refused its contacts"),
                _ => {}
            }
        }
        Ok((contacts, latest))
    }

    /// The channels set up in the first `slots` slots, each with its
    /// secret.
    pub async fn channels(&self, slots: u8) -> Vec<(u8, String, [u8; 16])> {
        let mut channels = Vec::new();
        for index in 0..slots {
            match self
                .ask_one(command::get_channel(index), |r| {
                    matches!(r, Response::ChannelInfo { .. })
                })
                .await
            {
                Ok(Response::ChannelInfo {
                    index,
                    name,
                    secret,
                }) if !name.is_empty() => {
                    channels.push((index, name, secret));
                }
                Ok(Response::ChannelInfo { .. }) => {}
                // Past the last slot the radio has.
                _ => break,
            }
        }
        channels
    }

    /// The next message queued, if any.
    pub async fn next_message(&self) -> Result<Option<Response>, &'static str> {
        let answer = self
            .ask_one(command::sync_next_message(), |r| {
                matches!(
                    r,
                    Response::ContactMessage(_)
                        | Response::ChannelMessage(_)
                        | Response::NoMoreMessages
                )
            })
            .await?;
        Ok(match answer {
            Response::NoMoreMessages | Response::Error(_) => None,
            message => Some(message),
        })
    }
}

/// The node number a public key stands for.
fn num_of(key: &[u8]) -> u32 {
    let mut first = [0u8; 4];
    first.copy_from_slice(&key[..4]);
    u32::from_be_bytes(first)
}

/// What a contact type is called, as roles are elsewhere.
fn role_name(kind: u8) -> String {
    match kind {
        1 => "CHAT".to_owned(),
        2 => "REPEATER".to_owned(),
        3 => "ROOM".to_owned(),
        4 => "SENSOR".to_owned(),
        other => other.to_string(),
    }
}

/// A position as MeshCore gives it, in millionths of a degree; none at zero.
fn position(lat: i32, lon: i32) -> Option<Position> {
    (lat != 0 || lon != 0).then(|| Position {
        latitude: f64::from(lat) / 1e6,
        longitude: f64::from(lon) / 1e6,
        altitude: None,
        radius: None,
        source: None,
    })
}

/// When the radio last heard of a contact. Not the time in its advert, which
/// is the sender's clock and may be anything (adverts from next year are
/// common), but when the radio itself last updated the contact, by its own
/// clock; never later than now.
fn last_heard(contact: &Contact) -> Option<u32> {
    let heard = match contact.last_modified {
        0 => contact.last_advert,
        updated => updated,
    };
    (heard > 0).then(|| heard.min(now()))
}

fn node_from(contact: &Contact) -> Node {
    Node {
        num: num_of(&contact.public_key),
        user: Some(User {
            long_name: contact.name.clone(),
            short_name: String::new(),
            hw_model: String::new(),
            role: role_name(contact.kind),
            public_key: contact.public_key.to_vec(),
            licensed: false,
            unmessagable: false,
        }),
        position: position(contact.lat, contact.lon),
        last_heard: last_heard(contact),
        snr: None,
        rssi: None,
        hops_away: hops(contact.path_len.cast_unsigned()),
        via_mqtt: false,
        favorite: contact.flags & 1 != 0,
        battery_level: None,
        voltage: None,
        channel_utilization: None,
        // A contact is what the radio keeps.
        on_radio: Some(true),
        status: None,
        status_at: None,
    }
}

/// Splits the `NAME: message` a MeshCore client writes, at the first
/// colon-space: whatever the person typed comes after it. A name too long or
/// spanning lines was punctuation, not a name, and the text is left whole.
fn split_sender(text: &str) -> Option<(&str, &str)> {
    let (name, body) = text.split_once(": ")?;
    if name.is_empty() || body.is_empty() || name.chars().count() > 32 || name.contains('\n') {
        return None;
    }
    Some((name, body))
}

/// An id for a message that has none of its own, so that the same message
/// heard twice (replayed on reconnecting, say) is known to be the same.
fn message_id(from: &[u8], timestamp: u32, text: &str) -> u32 {
    let mut hasher = DefaultHasher::new();
    from.hash(&mut hasher);
    timestamp.hash(&mut hasher);
    text.hash(&mut hasher);
    // The low half of the hash; zero is kept for "none".
    (u32::try_from(hasher.finish() & 0xffff_ffff).unwrap_or(1)).max(1)
}

/// How many hops a path has, from the byte the radio describes it with: the
/// low six bits count the hops, the top two say how many bytes each hop's
/// hash takes (on firmware that lets it be more than one). All ones is no
/// path known.
fn hops(path_len: u8) -> Option<u32> {
    (path_len != NO_PATH).then_some(u32::from(path_len & 0x3f))
}

/// A contact's path out: its length byte, and its hashes.
type Path = (u8, Vec<u8>);

/// A request waiting for its answer: who it went to, and what it asked.
type Asked = (u32, u8);

/// The open session with a radio: what sending needs.
#[derive(Clone)]
pub(crate) struct Session {
    client: Arc<Client>,
    /// Every contact's whole public key, by the node number it stands for.
    keys: Arc<Mutex<HashMap<u32, [u8; 32]>>>,
    /// Direct messages waiting for their acknowledgement: the tag the radio
    /// gave, and who they went to.
    pending: Arc<Mutex<HashMap<[u8; 4], u32>>>,
    /// Every contact's path out, its length byte and its hashes, by its node
    /// number.
    paths: Arc<Mutex<HashMap<u32, Path>>>,
    /// Requests waiting for their answer: the tag the radio gave, who they
    /// went to and what they asked.
    asked: Arc<Mutex<HashMap<[u8; 4], Asked>>>,
    /// Held while a message is sent with a region of its own, set before it
    /// and back after: another is not sent in between.
    sending: Arc<tokio::sync::Mutex<()>>,
}

/// What an anonymous request asks a node for, as the firmware numbers it.
pub mod anonymous {
    /// The regions it repeats on.
    pub const REGIONS: u8 = 0x01;
}

/// What a node answered a request with, read by what was asked: its first
/// four bytes are its clock.
fn answer_of(asked: u8, data: &[u8]) -> Option<crate::mesh::Answer> {
    let clock = data
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes);
    match asked {
        anonymous::REGIONS => {
            let text = String::from_utf8_lossy(data.get(4..).unwrap_or_default()).into_owned();
            let mut unscoped = false;
            let mut names = Vec::new();
            for name in text.trim_end_matches('\0').split(',') {
                let name = name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
                match name {
                    "" => {}
                    "*" => unscoped = true,
                    name => names.push(name.trim_start_matches('#').to_owned()),
                }
            }
            Some(crate::mesh::Answer::Regions {
                names,
                unscoped,
                clock,
            })
        }
        _ => None,
    }
}

impl Session {
    pub(crate) async fn send_text(
        &self,
        mesh: &Mesh,
        outgoing: &OutgoingText,
    ) -> Result<Sent, SendError> {
        let _sending = self.sending.lock().await;
        // A region of its own: set just before, and back just after, sent
        // or not.
        if let Some(scope) = outgoing.scope {
            let set = match scope {
                crate::mesh::Scope::Region(key) => command::set_flood_scope(Some(&key)),
                crate::mesh::Scope::Unscoped => command::set_flood_unscoped(),
            };
            self.client
                .done(set)
                .await
                .map_err(|_| SendError::Refused)?;
        }
        let sent = self.send_now(mesh, outgoing).await;
        if outgoing.scope.is_some()
            && let Err(error) = self.client.done(command::set_flood_scope(None)).await
        {
            tracing::warn!(?error, "the radio's region not set back");
        }
        sent
    }

    async fn send_now(&self, mesh: &Mesh, outgoing: &OutgoingText) -> Result<Sent, SendError> {
        let stamp = now();
        if outgoing.to == BROADCAST {
            let channel = u8::try_from(outgoing.channel).map_err(|_| SendError::Refused)?;
            let answer = self
                .client
                .ask_one(
                    command::send_channel_text(channel, stamp, &outgoing.text),
                    |r| matches!(r, Response::Ok),
                )
                .await
                .map_err(|_| SendError::NotConnected)?;
            if answer != Response::Ok {
                return Err(SendError::Refused);
            }
            // Nothing will ever say what became of it; an id of its own, as
            // every message kept needs one.
            let me = mesh.read(|s| s.local.as_ref().map_or(0, |l| l.num));
            return Ok(Sent {
                packet_id: message_id(&me.to_be_bytes(), stamp, &outgoing.text),
                acknowledged: false,
            });
        }
        let key = self
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&outgoing.to)
            .copied()
            .ok_or(SendError::Refused)?;
        let mut prefix = [0u8; 6];
        prefix.copy_from_slice(&key[..6]);
        let answer = self
            .client
            .ask_one(command::send_text(&prefix, stamp, &outgoing.text), |r| {
                matches!(r, Response::MessageSent { .. })
            })
            .await
            .map_err(|_| SendError::NotConnected)?;
        let Response::MessageSent { tag, timeout_ms } = answer else {
            return Err(SendError::Refused);
        };
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tag, outgoing.to);
        // Given up on if no acknowledgement comes in twice what the radio
        // suggests waiting.
        let timeout = Duration::from_millis(u64::from(timeout_ms).max(5_000) * 2);
        let pending = Arc::clone(&self.pending);
        let mesh = mesh.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let to = pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&tag);
            if let Some(to) = to {
                mesh.update(|_| {
                    vec![Event::Ack(Ack {
                        packet_id: u32::from_le_bytes(tag),
                        from: to,
                        error: Some("TIMEOUT".to_owned()),
                    })]
                });
            }
        });
        Ok(Sent {
            packet_id: u32::from_le_bytes(tag),
            acknowledged: true,
        })
    }
}

impl Mesh {
    /// The open session with a MeshCore radio.
    fn meshcore_session(&self) -> Result<Session, AdminError> {
        self.meshcore
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(AdminError::NotConnected)
    }

    /// Changes a MeshCore radio's own settings to `wanted`, each only if it
    /// changes, as the radio applies each at once; then reads them back, so
    /// that what is kept is what it took. A refusal stops there: what was
    /// changed before it stays changed.
    pub async fn meshcore_change(&self, wanted: &Settings) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        let client = &session.client;
        let current = self
            .read(|state| state.meshcore.clone())
            .ok_or(AdminError::NotConnected)?;
        if wanted.name != current.name {
            client.done(command::set_name(&wanted.name)).await?;
        }
        if (wanted.lat, wanted.lon) != (current.lat, current.lon) {
            client
                .done(command::set_position(wanted.lat, wanted.lon))
                .await?;
        }
        if wanted.tx_power != current.tx_power {
            client.done(command::set_tx_power(wanted.tx_power)).await?;
        }
        let radio = |s: &Settings| {
            (
                s.frequency,
                s.bandwidth,
                s.spreading_factor,
                s.coding_rate,
                s.repeat,
            )
        };
        if radio(wanted) != radio(&current) {
            // Repeat goes with them on firmware that has it, or it would
            // be turned off.
            let repeat = current.repeat.map(|now| wanted.repeat.unwrap_or(now));
            client
                .done(command::set_radio_params(
                    wanted.frequency,
                    wanted.bandwidth,
                    wanted.spreading_factor,
                    wanted.coding_rate,
                    repeat,
                ))
                .await?;
        }
        let other = |s: &Settings| (s.manual_add, s.telemetry, s.location_policy, s.multi_acks);
        if other(wanted) != other(&current) {
            client
                .done(command::set_other_params(
                    wanted.manual_add,
                    wanted.telemetry,
                    wanted.location_policy,
                    wanted.multi_acks,
                ))
                .await?;
        }
        if let (Some(mode), true) = (
            wanted.path_hash_mode,
            wanted.path_hash_mode != current.path_hash_mode,
        ) {
            client.done(command::set_path_hash_mode(mode)).await?;
        }
        if let (Some((rx_delay, airtime_factor)), true) =
            (wanted.tuning, wanted.tuning != current.tuning)
        {
            client
                .done(command::set_tuning(rx_delay, airtime_factor))
                .await?;
        }
        if let (Some((config, max_hops)), true) =
            (wanted.autoadd, wanted.autoadd != current.autoadd)
        {
            client.done(command::set_autoadd(config, max_hops)).await?;
        }
        if let (Some(scope), true) = (
            &wanted.default_scope,
            wanted.default_scope != current.default_scope,
        ) {
            client
                .done(command::set_default_scope(
                    scope.as_ref().map(|(name, key)| (name.as_str(), key)),
                ))
                .await?;
        }
        self.meshcore_read_back(client).await
    }

    /// Reads a MeshCore radio's settings back, and tells them as on syncing.
    async fn meshcore_read_back(&self, client: &Client) -> Result<(), AdminError> {
        let me = client.start().await.map_err(|_| AdminError::NoAnswer)?;
        let device = client.device().await;
        let extras = client.extras().await;
        let settings = Settings::from(&me, device.as_ref(), extras);
        self.update(|state| {
            if let Some(local) = state.local.as_mut() {
                describe(local, &settings);
            }
            let mut events = Vec::new();
            let own = state.local.as_ref().map(|l| l.num);
            if let Some(node) = own.and_then(|num| state.nodes.get_mut(&num)) {
                if let Some(user) = node.user.as_mut() {
                    user.long_name.clone_from(&settings.name);
                }
                node.position = position(settings.lat, settings.lon);
                events.push(Event::Node(node.clone()));
            }
            state.meshcore = Some(settings);
            events.push(Event::Synced);
            events
        });
        Ok(())
    }

    /// A MeshCore radio's figures, asked for now.
    pub async fn meshcore_status(&self) -> Result<Status, AdminError> {
        self.meshcore_session()?.client.status().await
    }

    /// Has the radio keep a contact, as a link gives it: its key, kind and
    /// name; no path known, nowhere. Updated if it was one already. Its
    /// node number.
    pub async fn meshcore_add_contact(
        &self,
        key: [u8; 32],
        kind: u8,
        name: &str,
    ) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let at = now();
        let contact = Contact {
            public_key: key,
            kind,
            flags: 0,
            path_len: -1,
            path: Vec::new(),
            name: name.to_owned(),
            last_advert: at,
            lat: 0,
            lon: 0,
            last_modified: at,
        };
        session.client.done(command::add_contact(&contact)).await?;
        remember_keys(&session, std::slice::from_ref(&contact));
        let node = node_from(&contact);
        let num = node.num;
        self.update(|state| {
            // What was known of it before, where it is above all, stays.
            let node = match state.nodes.get(&num) {
                Some(known) => Node {
                    user: node.user,
                    on_radio: Some(true),
                    ..known.clone()
                },
                None => node,
            };
            state.nodes.insert(num, node.clone());
            vec![Event::Node(node)]
        });
        Ok(num)
    }

    /// The whole key of a contact, by its node number.
    fn meshcore_key(&self, session: &Session, num: u32) -> Result<[u8; 32], AdminError> {
        session
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&num)
            .copied()
            .ok_or(AdminError::UnknownNode)
    }

    /// Has the radio forget a contact. Here it stays, as not in the radio's
    /// list.
    pub(crate) async fn meshcore_remove_contact(&self, num: u32) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        if self.read(|state| state.local.as_ref().map(|l| l.num)) == Some(num) {
            return Err(AdminError::OwnNode);
        }
        let key = self.meshcore_key(&session, num)?;
        session.client.done(command::remove_contact(&key)).await?;
        session
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&num);
        self.update(|state| match state.nodes.get_mut(&num) {
            Some(node) => {
                node.on_radio = Some(false);
                vec![Event::Node(node.clone())]
            }
            None => Vec::new(),
        });
        Ok(())
    }

    /// Has the radio forget the path to a contact: the next message to it
    /// floods to find one.
    pub async fn meshcore_reset_path(&self, num: u32) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        session.client.done(command::reset_path(&key)).await?;
        self.update(|state| match state.nodes.get_mut(&num) {
            Some(node) => {
                node.hops_away = None;
                vec![Event::Node(node.clone())]
            }
            None => Vec::new(),
        });
        Ok(())
    }

    /// Asks a node something with no login, as an anonymous request: it goes
    /// direct, on the path the radio knows, with the way back in it, as a
    /// node answers only those. The id it goes by, which its answer comes
    /// as a [`Event::Reply`] with.
    pub async fn meshcore_ask(&self, num: u32, kind: u8) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let (path_len, path) = session
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&num)
            .cloned()
            .ok_or(AdminError::UnknownNode)?;
        if path_len == NO_PATH {
            return Err(AdminError::NoPath);
        }
        // The way back: the same hashes, the other way round.
        let size = usize::from(path_len >> 6) + 1;
        let mut request = vec![kind, path_len];
        for hash in path.chunks(size).rev() {
            request.extend_from_slice(hash);
        }
        let answer = session
            .client
            .ask_one(command::send_anon_request(&key, &request), |r| {
                matches!(r, Response::MessageSent { .. })
            })
            .await;
        let tag = match answer {
            Ok(Response::MessageSent { tag, .. }) => tag,
            Ok(Response::Error(code)) => {
                return Err(AdminError::Refused(error_name(code).to_owned()));
            }
            Ok(_) => return Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        };
        session
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tag, (num, kind));
        Ok(u32::from_le_bytes(tag))
    }

    /// Has the radio say who it is: to those it hears directly, or flooded
    /// to the whole mesh.
    pub async fn meshcore_advert(&self, flood: bool) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        session.client.done(command::send_advert(flood)).await
    }

    /// Puts the channels in the first slots in the order given, each its
    /// name and secret, and empties the slots left; only those that change
    /// are written. Then reads them back, told as on syncing.
    pub async fn meshcore_reorder_channels(
        &self,
        order: &[(String, [u8; 16])],
    ) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        let (slots, now) = self
            .read(|state| {
                let meshcore = state.meshcore_channels.as_ref()?;
                let now: std::collections::BTreeMap<u32, (String, [u8; 16])> = state
                    .channels
                    .values()
                    .filter_map(|c| {
                        Some((c.index, (c.name.clone(), *meshcore.secrets.get(&c.index)?)))
                    })
                    .collect();
                Some((meshcore.slots, now))
            })
            .ok_or(AdminError::NotConnected)?;
        let mut wanted: Vec<Option<&(String, [u8; 16])>> = order.iter().map(Some).collect();
        wanted.resize(usize::from(slots), None);
        let mut result = Ok(());
        for (index, channel) in (0..slots).zip(wanted) {
            let had = now.get(&u32::from(index));
            if had == channel {
                continue;
            }
            let (name, secret) = channel.map_or(("", &[0; 16]), |(n, s)| (n.as_str(), s));
            if had.is_none() && name.is_empty() {
                continue;
            }
            result = session
                .client
                .done(command::set_channel(index, name, secret))
                .await;
            if result.is_err() {
                break;
            }
        }
        // Read back whatever came of it, all or part.
        let channels = session.client.channels(slots).await;
        self.update(|state| {
            put_channels(state, slots, channels);
            vec![Event::Synced]
        });
        result
    }

    /// Sets a channel in a slot, or empties it with no name; then reads the
    /// channels back, told as on syncing.
    pub async fn meshcore_set_channel(
        &self,
        index: u8,
        name: &str,
        secret: &[u8; 16],
    ) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        let slots = self
            .read(|state| state.meshcore_channels.as_ref().map(|c| c.slots))
            .ok_or(AdminError::NotConnected)?;
        session
            .client
            .done(command::set_channel(index, name, secret))
            .await?;
        let channels = session.client.channels(slots).await;
        self.update(|state| {
            put_channels(state, slots, channels);
            vec![Event::Synced]
        });
        Ok(())
    }
}

/// Connects to the radio and keeps connected, for as long as the task runs.
pub(crate) async fn run(mesh: Mesh, transport: Transport) {
    let mut delay = RECONNECT_MIN;
    loop {
        mesh.set_link(LinkState::Connecting);
        match Client::connect(&transport).await {
            Ok((client, pushes)) => {
                tracing::info!(radio = %transport, "connected");
                let started = Instant::now();
                let why = session(&mesh, Arc::new(client), pushes).await;
                *mesh.meshcore.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if started.elapsed() >= STABLE_SESSION {
                    delay = RECONNECT_MIN;
                }
                tracing::warn!(radio = %transport, retry_in = delay.as_secs(), "{why}, reconnecting");
            }
            Err(error) => {
                tracing::warn!(radio = %transport, %error, retry_in = delay.as_secs(), "cannot connect");
            }
        }
        mesh.set_link(LinkState::Disconnected);
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RECONNECT_MAX);
    }
}

/// One connection, from saying who we are until it drops. Returns why it
/// ended.
async fn session(
    mesh: &Mesh,
    client: Arc<Client>,
    mut pushes: mpsc::UnboundedReceiver<Response>,
) -> &'static str {
    mesh.set_link(LinkState::Syncing);
    let me = match client.start().await {
        Ok(me) => me,
        Err(why) => return why,
    };
    let device = client.device().await;
    client.keep_time().await;
    let extras = client.extras().await;
    let settings = Settings::from(&me, device.as_ref(), extras);
    let mut local = LocalNode {
        num: num_of(&me.public_key),
        firmware_version: device.as_ref().and_then(|d| match (&d.version, &d.build) {
            (Some(version), Some(build)) => Some(format!("{version} ({build})")),
            (version, _) => version.clone(),
        }),
        hw_model: device.as_ref().and_then(|d| d.model.clone()),
        role: Some(role_name(me.kind)),
        region: None,
        modem_preset: None,
    };
    describe(&mut local, &settings);
    let slots = device
        .as_ref()
        .and_then(|d| d.max_channels)
        .unwrap_or(CHANNELS_DEFAULT);

    let (contacts, mut last_change) = match client.contacts(0).await {
        Ok(read) => read,
        Err(why) => return why,
    };
    let channels = client.channels(slots).await;
    let session = Session {
        client: Arc::clone(&client),
        keys: Arc::default(),
        pending: Arc::default(),
        paths: Arc::default(),
        asked: Arc::default(),
        sending: Arc::default(),
    };
    let me_node = Node {
        last_heard: Some(now()),
        hops_away: Some(0),
        ..node_from(&Contact {
            public_key: me.public_key,
            kind: me.kind,
            flags: 0,
            path_len: 0,
            path: Vec::new(),
            name: me.name.clone(),
            last_advert: now(),
            lat: me.lat,
            lon: me.lon,
            last_modified: now(),
        })
    };
    remember_keys(&session, &contacts);
    mesh.update(|state| {
        state.local = Some(local);
        state.meshcore = Some(settings);
        put_channels(state, slots, channels);
        // Nodes heard so far are kept, as with Meshtastic; these join them.
        for node in contacts.iter().map(node_from).chain([me_node]) {
            state.named.insert(node.num);
            state.nodes.insert(node.num, node);
        }
        Vec::new()
    });
    *mesh.meshcore.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.clone());

    // Whatever arrived while nobody was connected: announced once, to nobody.
    if let Err(why) = drain(mesh, &client).await {
        return why;
    }
    let nodes = mesh.read(|s| s.nodes.len());
    tracing::info!(nodes, "in sync with the radio");
    mesh.set_link(LinkState::Ready);

    let mut contacts_due: Option<Instant> = None;
    let mut reread = tokio::time::interval_at(Instant::now() + REREAD_INTERVAL, REREAD_INTERVAL);
    loop {
        let settle = async {
            match contacts_due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            push = pushes.recv() => match push {
                None => return "the link closed",
                Some(Response::MessagesWaiting) => {
                    if let Err(why) = drain(mesh, &client).await {
                        return why;
                    }
                }
                Some(Response::Advert | Response::PathUpdate) => {
                    contacts_due.get_or_insert(Instant::now() + CONTACTS_SETTLE);
                }
                Some(Response::NewContact(contact)) => {
                    remember_keys(&session, std::slice::from_ref(&contact));
                    let node = node_from(&contact);
                    mesh.update(|state| {
                        state.nodes.insert(node.num, node.clone());
                        vec![Event::Node(node)]
                    });
                }
                Some(Response::Ack(tag)) => acknowledged(mesh, &session, tag),
                Some(Response::BinaryResponse { tag, data }) => {
                    replied(mesh, &session, tag, &data);
                }
                Some(_) => {}
            },
            // Every contact again, now and then: those the radio no longer
            // has, as another client removed them, go out of its list here.
            _ = reread.tick() => {
                match client.contacts(0).await {
                    Ok((all, latest)) => {
                        last_change = latest;
                        remember_keys(&session, &all);
                        mesh.update(|state| {
                            let me = state.local.as_ref().map(|l| l.num);
                            state.named = all
                                .iter()
                                .map(|c| num_of(&c.public_key))
                                .chain(me)
                                .collect();
                            for node in all.iter().map(node_from) {
                                state.nodes.insert(node.num, node);
                            }
                            state.read_through();
                            vec![Event::Synced]
                        });
                    }
                    Err(why) => tracing::debug!(why, "contacts not read again"),
                }
            }
            () = settle => {
                contacts_due = None;
                match client.contacts(last_change).await {
                    Ok((changed, latest)) => {
                        last_change = latest;
                        remember_keys(&session, &changed);
                        mesh.update(|state| {
                            changed
                                .iter()
                                .map(node_from)
                                .map(|node| {
                                    state.nodes.insert(node.num, node.clone());
                                    Event::Node(node)
                                })
                                .collect()
                        });
                    }
                    Err(why) => tracing::debug!(why, "contacts not read again"),
                }
            }
        }
    }
}

/// A MeshCore radio's channels, as read: their names where every radio's
/// are, their secrets apart.
fn put_channels(state: &mut MeshState, slots: u8, channels: Vec<(u8, String, [u8; 16])>) {
    let mut secrets = std::collections::BTreeMap::new();
    state.channels = channels
        .into_iter()
        .map(|(index, name, secret)| {
            let index = u32::from(index);
            secrets.insert(index, secret);
            let role = if index == 0 { "PRIMARY" } else { "SECONDARY" };
            (
                index,
                Channel {
                    index,
                    name,
                    role: role.to_owned(),
                },
            )
        })
        .collect();
    state.meshcore_channels = Some(Channels { slots, secrets });
}

/// Asks for every queued message until there are none.
async fn drain(mesh: &Mesh, client: &Client) -> Result<(), &'static str> {
    while let Some(message) = client.next_message().await? {
        match message {
            Response::ContactMessage(message) => mesh.update(|state| direct(state, &message)),
            Response::ChannelMessage(message) => mesh.update(|state| channel(state, &message)),
            _ => {}
        }
    }
    Ok(())
}

fn acknowledged(mesh: &Mesh, session: &Session, tag: [u8; 4]) {
    let to = session
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&tag);
    if let Some(to) = to {
        mesh.update(|_| {
            vec![Event::Ack(Ack {
                packet_id: u32::from_le_bytes(tag),
                from: to,
                error: None,
            })]
        });
    }
}

fn remember_keys(session: &Session, contacts: &[Contact]) {
    let mut keys = session.keys.lock().unwrap_or_else(|e| e.into_inner());
    let mut paths = session.paths.lock().unwrap_or_else(|e| e.into_inner());
    for contact in contacts {
        let num = num_of(&contact.public_key);
        keys.insert(num, contact.public_key);
        paths.insert(
            num,
            (contact.path_len.cast_unsigned(), contact.path.clone()),
        );
    }
}

/// A node's answer to a request, told if it was asked.
fn replied(mesh: &Mesh, session: &Session, tag: [u8; 4], data: &[u8]) {
    let asked = session
        .asked
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&tag);
    let Some((from, kind)) = asked else {
        return;
    };
    let Some(answer) = answer_of(kind, data) else {
        tracing::debug!(from, kind, "unreadable answer");
        return;
    };
    mesh.update(|_| {
        vec![Event::Reply(crate::mesh::Reply {
            request_id: u32::from_le_bytes(tag),
            from,
            answer,
        })]
    });
}

fn direct(state: &mut MeshState, message: &ContactMessage) -> Vec<Event> {
    let from = num_of(&message.prefix);
    let at = now();
    let mut events = Vec::new();
    if let Some(node) = state.nodes.get_mut(&from) {
        node.last_heard = Some(at);
        node.hops_away = hops(message.path_len);
        if message.snr.is_some() {
            node.snr = message.snr;
        }
        events.push(Event::Node(node.clone()));
    }
    events.push(Event::Message(TextMessage {
        packet_id: message_id(&message.prefix, message.timestamp, &message.text),
        from,
        to: state.local.as_ref().map_or(0, |l| l.num),
        channel: 0,
        text: message.text.clone(),
        at,
        reply_id: None,
        sender_name: None,
        emoji: false,
        snr: message.snr,
        rssi: None,
        hops_away: hops(message.path_len),
        via_mqtt: false,
    }));
    events
}

fn channel(state: &mut MeshState, message: &ChannelMessage) -> Vec<Event> {
    let (name, text) = match split_sender(&message.text) {
        Some((name, body)) => (Some(name.to_owned()), body.to_owned()),
        None => (None, message.text.clone()),
    };
    // The name stands for a node only when exactly one contact has it.
    let from = name
        .as_deref()
        .and_then(|name| {
            let mut named = state
                .nodes
                .values()
                .filter(|n| n.user.as_ref().is_some_and(|u| u.long_name == name));
            match (named.next(), named.next()) {
                (Some(node), None) => Some(node.num),
                _ => None,
            }
        })
        .unwrap_or(0);
    vec![Event::Message(TextMessage {
        packet_id: message_id(&[message.channel], message.timestamp, &message.text),
        from,
        to: BROADCAST,
        channel: u32::from(message.channel),
        text,
        at: now(),
        reply_id: None,
        sender_name: name,
        emoji: false,
        snr: message.snr,
        rssi: None,
        hops_away: hops(message.path_len),
        via_mqtt: false,
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(key_start: u8, name: &str, kind: u8) -> Contact {
        let mut public_key = [0u8; 32];
        public_key[0] = key_start;
        Contact {
            public_key,
            kind,
            flags: 0,
            path_len: 2,
            path: vec![0x11, 0x22],
            name: name.into(),
            last_advert: 1_795_000_000,
            lat: 43_331_140,
            lon: -8_463_732,
            last_modified: 1,
        }
    }

    #[test]
    fn a_contact_is_a_node() {
        let node = node_from(&contact(0x50, "Faro", 2));
        assert_eq!(node.num, 0x5000_0000);
        assert_eq!(node.id(), "!50000000");
        let user = node.user.unwrap();
        assert_eq!(user.long_name, "Faro");
        assert_eq!(user.role, "REPEATER");
        assert_eq!(node.hops_away, Some(2));
        let position = node.position.unwrap();
        assert!((position.latitude - 43.33114).abs() < 1e-9);
    }

    #[test]
    fn heard_is_by_the_radio_clock_and_never_ahead() {
        let mut heard = contact(1, "Faro", 1);
        heard.last_advert = 4_000_000_000;
        heard.last_modified = 1_795_000_000;
        assert_eq!(last_heard(&heard), Some(1_795_000_000));
        heard.last_modified = 4_000_000_000;
        assert!(last_heard(&heard).unwrap() <= now());
    }

    #[test]
    fn an_unknown_path_is_no_hops() {
        let mut far = contact(1, "Lejos", 1);
        far.path_len = -1;
        assert_eq!(node_from(&far).hops_away, None);
        assert_eq!(hops(NO_PATH), None);
    }

    #[test]
    fn regions_read_as_a_repeater_answers_them() {
        // As StaLocaia answered: its clock, then the names, padded.
        let mut data = 1_718_789_276u32.to_le_bytes().to_vec();
        data.extend(b"*,europe,#galicia,\0\0\0\0");
        assert_eq!(
            answer_of(anonymous::REGIONS, &data),
            Some(crate::mesh::Answer::Regions {
                names: vec!["europe".into(), "galicia".into()],
                unscoped: true,
                clock: Some(1_718_789_276),
            })
        );
        // None at all, and what was not asked.
        let none = 1u32.to_le_bytes();
        assert_eq!(
            answer_of(anonymous::REGIONS, &none),
            Some(crate::mesh::Answer::Regions {
                names: Vec::new(),
                unscoped: false,
                clock: Some(1),
            })
        );
        assert_eq!(answer_of(0x7f, &none), None);
    }

    #[test]
    fn hops_leave_the_hash_size_out() {
        // Heard on a real radio: four hops of two-byte hashes.
        assert_eq!(hops(68), Some(4));
        assert_eq!(hops(10), Some(10));
        // Four-byte hashes set the top bit, which is not "unknown".
        let mut far = contact(1, "Lejos", 1);
        far.path_len = 0xc3u8.cast_signed();
        assert_eq!(node_from(&far).hops_away, Some(3));
    }

    #[test]
    fn the_sender_is_read_from_the_text() {
        assert_eq!(
            split_sender("ALICE: hola: qué tal"),
            Some(("ALICE", "hola: qué tal"))
        );
        assert_eq!(split_sender("https://example.org"), None);
        assert_eq!(split_sender("sin nombre"), None);
    }

    fn channel_message(text: &str) -> ChannelMessage {
        ChannelMessage {
            channel: 1,
            path_len: 3,
            text_type: 0,
            timestamp: 42,
            text: text.into(),
            snr: Some(4.5),
        }
    }

    #[test]
    fn a_channel_message_names_its_sender_and_finds_them() {
        let mut state = MeshState::default();
        let faro = node_from(&contact(0x50, "Faro", 1));
        state.nodes.insert(faro.num, faro);
        let events = channel(&mut state, &channel_message("Faro: buenas"));
        let [Event::Message(message)] = events.as_slice() else {
            panic!("a message");
        };
        assert_eq!(message.text, "buenas");
        assert_eq!(message.sender_name.as_deref(), Some("Faro"));
        assert_eq!(message.from, 0x5000_0000);
        assert_eq!(message.to, BROADCAST);
        assert_eq!(message.channel, 1);
        assert_eq!(message.hops_away, Some(3));
    }

    #[test]
    fn a_name_two_contacts_share_stands_for_neither() {
        let mut state = MeshState::default();
        for start in [1, 2] {
            let node = node_from(&contact(start, "Faro", 1));
            state.nodes.insert(node.num, node);
        }
        let events = channel(&mut state, &channel_message("Faro: buenas"));
        let [Event::Message(message)] = events.as_slice() else {
            panic!("a message");
        };
        assert_eq!(message.from, 0);
        assert_eq!(message.sender_name.as_deref(), Some("Faro"));
    }

    #[test]
    fn the_same_message_heard_twice_is_the_same() {
        let mut state = MeshState::default();
        let once = channel(&mut state, &channel_message("Faro: buenas"));
        let twice = channel(&mut state, &channel_message("Faro: buenas"));
        let id = |events: &[Event]| match events {
            [Event::Message(m)] => m.packet_id,
            _ => panic!("a message"),
        };
        assert_eq!(id(&once), id(&twice));
    }

    #[test]
    fn a_direct_message_comes_from_a_contact() {
        let mut state = MeshState {
            local: Some(LocalNode {
                num: 7,
                ..LocalNode::default()
            }),
            ..MeshState::default()
        };
        let faro = node_from(&contact(0x50, "Faro", 1));
        state.nodes.insert(faro.num, faro);
        let message = ContactMessage {
            prefix: [0x50, 0, 0, 0, 0, 0],
            path_len: NO_PATH,
            text_type: 0,
            timestamp: 42,
            text: "hola".into(),
            snr: Some(6.0),
        };
        let events = direct(&mut state, &message);
        assert!(matches!(&events[0], Event::Node(n) if n.snr == Some(6.0)));
        let Event::Message(message) = &events[1] else {
            panic!("a message");
        };
        assert_eq!(message.from, 0x5000_0000);
        assert_eq!(message.to, 7);
        assert_eq!(message.hops_away, None);
    }
}
