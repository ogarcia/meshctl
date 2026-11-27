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

pub mod answers;
pub mod protocol;
pub mod raw;

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
    Access, ChannelMessage, Contact, ContactMessage, DeviceInfo, LoggedIn, Response, SelfInfo,
    Stats, command,
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
    /// Whether its GPS is on, which then says where it is; none on a radio
    /// with no GPS, or firmware without it.
    pub gps: Option<bool>,
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
            gps: extras.gps,
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
    /// Whether its GPS is on.
    pub gps: Option<bool>,
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

    /// The seconds it has spent sending and receiving since it started, as
    /// its radio's figures say; none if its firmware has none.
    pub async fn airtime(&self) -> Result<Option<(u32, u32)>, AdminError> {
        match self
            .ask_one(command::get_stats(1), |r| matches!(r, Response::Stats(_)))
            .await
        {
            Ok(Response::Stats(Stats::Radio {
                tx_air_secs,
                rx_air_secs,
                ..
            })) => Ok(Some((tx_air_secs, rx_air_secs))),
            Ok(_) => Ok(None),
            Err("the radio did not answer") => Err(AdminError::NoAnswer),
            Err(_) => Err(AdminError::NotConnected),
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
        if let Ok(Response::CustomVars(vars)) = self
            .ask_one(command::get_custom_vars(), |r| {
                matches!(r, Response::CustomVars(_))
            })
            .await
        {
            extras.gps = vars
                .iter()
                .find(|(name, _)| name == "gps")
                .map(|(_, value)| value == "1");
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

/// A contact's kind, from the role [`role_name`] names: a chat's if it is
/// none of the others.
fn kind_of(role: &str) -> u8 {
    match role {
        "REPEATER" => 2,
        "ROOM" => 3,
        "SENSOR" => 4,
        _ => 1,
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
/// What was asked of a node, by the tag it went by: of whom, what, and
/// where to say it was answered, which frees the radio for the next.
struct Asked {
    from: u32,
    what: Asking,
    answered: Option<tokio::sync::oneshot::Sender<()>>,
}

/// A trace sent: to whom, when, and for a traceroute the hops it goes
/// along, by their hashes; none for a ping.
type Pinged = (u32, std::time::Instant, Option<Vec<Vec<u8>>>);

/// What a request asks: anonymously, or of a node logged in to, as the
/// firmware numbers each; the way to a node and back; or what a sensor kept
/// over so many seconds back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asking {
    Anonymous(u8),
    LoggedIn(u8),
    Route,
    Series(u32),
}

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
    /// Held while the radio waits for an answer to a request or a login:
    /// it waits for one at a time, and another would make it forget it.
    waiting: Arc<tokio::sync::Mutex<()>>,
    /// Pings waiting for their trace back, by its tag: to whom, and when.
    pinged: Arc<Mutex<HashMap<[u8; 4], Pinged>>>,
    /// Held while a message is sent with a region of its own, set before it
    /// and back after: another is not sent in between.
    sending: Arc<tokio::sync::Mutex<()>>,
    /// The packets heard lately, to tell which message each copy is.
    heard: Arc<Mutex<Heard>>,
    /// Logins waiting for their answer, by the first six bytes of the key of
    /// the node they went to: what it let in as, or none if it refused.
    logins: Arc<Mutex<HashMap<[u8; 6], LoginWait>>>,
}

/// Where a login's answer is told: what the node let in as, or none.
type LoginWait = tokio::sync::oneshot::Sender<Option<Access>>;

/// How long a packet heard is remembered, for its later copies.
const HEARD_KEPT: Duration = Duration::from_secs(600);
/// The least a login waits for its answer: a node far off, flooded to,
/// takes a while to.
const LOGIN_WAIT: Duration = Duration::from_secs(15);
/// How long a direct text heard waits for the message it is.
const DIRECT_WAIT: Duration = Duration::from_secs(30);

/// The packets a radio heard lately: those known, by what they carry, and
/// the message each is; the direct texts to it that wait for their message;
/// and the direct messages come before any copy of them, by their sender's
/// hash, that wait for theirs.
#[derive(Default)]
struct Heard {
    known: HashMap<u64, (u32, Instant)>,
    waiting: std::collections::VecDeque<HeardCopy>,
    unheard: std::collections::VecDeque<(u8, u32, Instant)>,
}

/// A copy of a packet heard, as the radio logged it.
#[derive(Clone)]
struct HeardCopy {
    packet: raw::RawPacket,
    snr: f32,
    rssi: i8,
    at: u32,
    heard: Instant,
}

impl HeardCopy {
    fn route(&self, packet_id: u32) -> crate::mesh::Route {
        crate::mesh::Route {
            packet_id,
            hops: self.packet.hops(),
            flooded: self.packet.flooded(),
            snr: self.snr,
            rssi: i32::from(self.rssi),
            at: self.at,
            region: self.packet.transport.map(|codes| crate::mesh::RegionCode {
                code: codes[0],
                payload_type: self.packet.payload_type,
                payload: self.packet.payload.clone(),
            }),
        }
    }
}

/// A packet the radio heard, raw: told as the message it is a copy of, if
/// that can be said now; a direct text to it waits for its message.
/// A copy of a packet heard raw, read as far as the radio's channels and
/// contacts let: for the live view.
fn raw_heard(
    state: &MeshState,
    packet: &raw::RawPacket,
    snr: f32,
    rssi: i8,
) -> crate::mesh::RawHeard {
    use crate::mesh::RawInfo;
    let payload = &packet.payload;
    let info = match packet.payload_type {
        raw::PAYLOAD_ADVERT => {
            raw::advert(payload).map_or(RawInfo::Other, |advert| RawInfo::Advert {
                node: num_of(&advert.key),
                kind: advert.kind,
                name: advert.name,
                position: advert.position,
                timestamp: advert.timestamp,
            })
        }
        raw::PAYLOAD_GRP_TXT => {
            let opened = state.meshcore_channels.as_ref().and_then(|channels| {
                channels.secrets.iter().find_map(|(index, secret)| {
                    raw::open_channel_text(secret, payload).map(|(at, text)| (*index, at, text))
                })
            });
            match opened {
                Some((channel, timestamp, text)) => {
                    let (sender, text) = match split_sender(&text) {
                        Some((name, body)) => (Some(name.to_owned()), body.to_owned()),
                        None => (None, text),
                    };
                    RawInfo::ChannelText {
                        channel,
                        sender,
                        text,
                        timestamp,
                    }
                }
                None => RawInfo::Sealed {
                    channel_hash: payload.first().copied().unwrap_or(0),
                },
            }
        }
        // Between two nodes: whom to and from, by a byte of each one's key.
        0x00 | 0x01 | raw::PAYLOAD_TXT_MSG | 0x08 => match (payload.first(), payload.get(1)) {
            (Some(&to), Some(&from)) => RawInfo::Between { to, from },
            _ => RawInfo::Other,
        },
        raw::PAYLOAD_ANON_REQ => match (payload.first(), payload.get(1..33)) {
            (Some(&to), Some(key)) => RawInfo::Anonymous {
                to,
                from: num_of(key),
            },
            _ => RawInfo::Other,
        },
        raw::PAYLOAD_ACK => {
            payload
                .get(..4)
                .and_then(|b| b.try_into().ok())
                .map_or(RawInfo::Other, |b| RawInfo::Ack {
                    code: u32::from_le_bytes(b),
                })
        }
        // Its tag, a code, its flags, whose lowest two bits tell the size of
        // each hash of the path after them: 1, 2, 4 or 8 bytes.
        raw::PAYLOAD_TRACE => match (payload.get(..4), payload.get(8)) {
            (Some(tag), Some(flags)) => RawInfo::Trace {
                tag: u32::from_le_bytes(tag.try_into().unwrap_or_default()),
                hashes: payload.get(9..).unwrap_or_default().to_vec(),
                hash_size: 1 << (flags & 0x03),
            },
            _ => RawInfo::Other,
        },
        _ => RawInfo::Other,
    };
    crate::mesh::RawHeard {
        key: packet.key(),
        payload_type: packet.payload_type,
        route: packet.route,
        region: packet.transport.map(|codes| crate::mesh::RegionCode {
            code: codes[0],
            payload_type: packet.payload_type,
            payload: packet.payload.clone(),
        }),
        path: packet.path.clone(),
        hash_size: (packet.path_len >> 6) + 1,
        payload_len: payload.len(),
        snr,
        rssi: i32::from(rssi),
        at: now(),
        info,
    }
}

fn heard_raw(mesh: &Mesh, session: &Session, snr: f32, rssi: i8, bytes: &[u8]) {
    let Some(packet) = raw::RawPacket::parse(bytes) else {
        return;
    };
    // For the live view, every copy, read as far as it can be.
    mesh.update(|state| vec![Event::RawHeard(raw_heard(state, &packet, snr, rssi))]);
    let now = Instant::now();
    let copy = HeardCopy {
        packet,
        snr,
        rssi,
        at: crate::meshcore::now(),
        heard: now,
    };
    let key = copy.packet.key();
    let mut heard = session.heard.lock().unwrap_or_else(|e| e.into_inner());
    heard
        .known
        .retain(|_, (_, at)| now.duration_since(*at) < HEARD_KEPT);
    heard
        .waiting
        .retain(|c| now.duration_since(c.heard) < DIRECT_WAIT);
    heard
        .unheard
        .retain(|(_, _, at)| now.duration_since(*at) < DIRECT_WAIT);
    let known = heard.known.get(&key).map(|(id, _)| *id);
    let packet_id = match (known, copy.packet.payload_type) {
        (Some(id), _) => Some(id),
        (None, raw::PAYLOAD_GRP_TXT) => {
            let id = mesh.read(|state| channel_packet_id(state, &copy.packet.payload));
            if let Some(id) = id {
                heard.known.insert(key, (id, now));
            }
            id
        }
        (None, raw::PAYLOAD_TXT_MSG) => {
            // To the radio: kept for the message from its sender.
            let own = mesh.read(|state| state.local.as_ref().map(|l| l.num.to_be_bytes()[0]));
            if copy.packet.payload.first().copied() != own {
                None
            } else {
                // Its message come already, from its sender: it is that one.
                let from = copy.packet.payload.get(1).copied();
                match heard.unheard.iter().position(|(f, _, _)| Some(*f) == from) {
                    Some(at) => {
                        let id = heard.unheard.remove(at).map_or(0, |(_, id, _)| id);
                        heard.known.insert(key, (id, now));
                        Some(id)
                    }
                    None => {
                        heard.waiting.push_back(copy.clone());
                        None
                    }
                }
            }
        }
        _ => None,
    };
    drop(heard);
    if let Some(id) = packet_id {
        mesh.update(|_| vec![Event::Route(copy.route(id))]);
    }
}

/// The message a channel text heard raw is, opened with the secret of a
/// channel the radio has: as heard, or ours, as repeaters repeat it.
fn channel_packet_id(state: &MeshState, payload: &[u8]) -> Option<u32> {
    let channels = state.meshcore_channels.as_ref()?;
    let (index, timestamp, text) = channels.secrets.iter().find_map(|(index, secret)| {
        raw::open_channel_text(secret, payload).map(|(at, text)| (*index, at, text))
    })?;
    let own = state.meshcore.as_ref().map(|s| format!("{}: ", s.name));
    let me = state.local.as_ref().map(|l| l.num);
    match (own, me) {
        (Some(own), Some(me)) if text.starts_with(&own) => {
            Some(message_id(&me.to_be_bytes(), timestamp, &text[own.len()..]))
        }
        _ => Some(message_id(
            &[u8::try_from(index).unwrap_or(0)],
            timestamp,
            &text,
        )),
    }
}

/// A direct message come: the copies of it heard raw, from its sender, are
/// told as it, and so will later ones be.
fn matched_direct(mesh: &Mesh, session: &Session, from: u8, packet_id: u32) {
    let mut heard = session.heard.lock().unwrap_or_else(|e| e.into_inner());
    // The latest from its sender, and every copy of the same.
    let Some(key) = heard
        .waiting
        .iter()
        .rev()
        .find(|c| c.packet.payload.get(1).copied() == Some(from))
        .map(|c| c.packet.key())
    else {
        // Its copy may come yet, logged ahead of it but read after.
        heard.unheard.push_back((from, packet_id, Instant::now()));
        return;
    };
    let copies: Vec<HeardCopy> = heard
        .waiting
        .iter()
        .filter(|c| c.packet.key() == key)
        .cloned()
        .collect();
    heard.waiting.retain(|c| c.packet.key() != key);
    heard.known.insert(key, (packet_id, Instant::now()));
    drop(heard);
    mesh.update(|_| {
        copies
            .iter()
            .map(|c| Event::Route(c.route(packet_id)))
            .collect()
    });
}

/// What a request asks a node logged in to, as the firmware numbers it.
pub mod request {
    /// How it is doing, a repeater or a room: guests may ask.
    pub const STATUS: u8 = 0x01;
    /// What its sensors read; a guest, only its own.
    pub const TELEMETRY: u8 = 0x03;
    /// What a sensor kept of its readings over a time: the lowest, the
    /// highest and the mean of each. One logged in to read, at least.
    pub const SERIES: u8 = 0x04;
    /// Who may log in to it, and as what: only its admin may ask.
    pub const ACCESS_LIST: u8 = 0x05;
    /// The nodes it hears straight.
    pub const NEIGHBOURS: u8 = 0x06;
    /// Its firmware, its name and its owner.
    pub const OWNER: u8 = 0x07;
}

/// How many neighbours are asked for at once: as many as an answer holds,
/// each in nine bytes.
const NEIGHBOURS_ASKED: u8 = 14;

/// The least a request waits for its answer before the radio is let ask
/// another.
const ASK_WAIT: Duration = Duration::from_secs(15);

/// The least a ping waits for its trace back before it is given up on: one
/// heard straight comes back in a second or two.
const PING_WAIT: Duration = Duration::from_secs(10);

/// A ping's tag: of whom and when, as a trace goes by one of its own.
fn ping_tag(num: u32) -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    num.rotate_left(7) ^ nanos
}

/// What an anonymous request asks a node for, as the firmware numbers it.
pub mod anonymous {
    /// The regions it repeats on.
    pub const REGIONS: u8 = 0x01;
    /// Its name and what its owner says, with no firmware: of one heard
    /// straight, or on a path known, with no login.
    pub const OWNER: u8 = 0x02;
}

/// What a node answered a request with, read by what was asked: an
/// anonymous answer starts with its clock; one logged in to, with nothing
/// before. A room's status is a room's.
fn answer_of(asked: Asking, data: &[u8], room: bool) -> Option<crate::mesh::Answer> {
    use crate::mesh::Answer;
    let asked = match asked {
        Asking::LoggedIn(request::STATUS) => {
            return answers::status(data, room).map(Answer::Status);
        }
        Asking::LoggedIn(request::TELEMETRY) => {
            return Some(Answer::Telemetry(answers::telemetry(data)));
        }
        Asking::LoggedIn(request::NEIGHBOURS) => {
            let (total, heard) = answers::neighbours(data, 4)?;
            let heard = heard
                .into_iter()
                .map(|n| crate::mesh::Neighbour {
                    node: num_of(&n.prefix),
                    seconds_ago: n.seconds_ago,
                    snr: n.snr,
                })
                .collect();
            return Some(Answer::Neighbours { total, heard });
        }
        Asking::LoggedIn(request::OWNER) => return answers::owner(data).map(Answer::Owner),
        // Each one by the first six bytes of its key, and its bits.
        Asking::LoggedIn(request::ACCESS_LIST) => {
            let listed = data
                .as_chunks::<7>()
                .0
                .iter()
                .map(|entry| (num_of(&entry[..6]), entry[6]))
                .collect();
            return Some(Answer::AccessList(listed));
        }
        Asking::Series(seconds) => {
            let (clock, readings) = answers::series(data)?;
            return Some(Answer::Series {
                seconds,
                clock,
                readings,
            });
        }
        // Its answer comes as no binary one.
        Asking::LoggedIn(_) | Asking::Route => return None,
        Asking::Anonymous(kind) => kind,
    };
    let clock = data
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes);
    match asked {
        // Its name and its owner's words, after its clock: no firmware.
        anonymous::OWNER => {
            let said = answers::owner(&[b"\n", data.get(4..)?].concat())?;
            Some(crate::mesh::Answer::Owner(said))
        }
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
        let stamp = outgoing.sent_at.unwrap_or_else(now);
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
                sent_at: Some(stamp),
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
            sent_at: Some(stamp),
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
        // Its GPS last: on, it says where the radio is from then on.
        if let (Some(on), true) = (wanted.gps, wanted.gps != current.gps) {
            client
                .done(command::set_custom_var("gps", if on { "1" } else { "0" }))
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

    /// The seconds a MeshCore radio has spent sending and receiving since it
    /// started, asked for now; none if its firmware does not say.
    pub async fn meshcore_airtime(&self) -> Result<Option<(u32, u32)>, AdminError> {
        self.meshcore_session()?.client.airtime().await
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

    /// Has the radio keep a node it does not, which it was heard of: one
    /// that advertised and it did not add, or that it was had forget. As
    /// a link adds one: its key, kind and name, no path known.
    pub async fn meshcore_keep(&self, num: u32) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        if self.read(|state| state.local.as_ref().map(|l| l.num)) == Some(num) {
            return Err(AdminError::OwnNode);
        }
        let (kind, name, known) = self
            .read(|state| {
                let user = state.nodes.get(&num)?.user.as_ref()?;
                Some((
                    kind_of(&user.role),
                    user.long_name.clone(),
                    <[u8; 32]>::try_from(user.public_key.as_slice()).ok(),
                ))
            })
            .ok_or(AdminError::UnknownNode)?;
        // The session's, as heard; or, had forget, the one kept of it.
        let key = match self.meshcore_key(&session, num) {
            Ok(key) => key,
            Err(_) => known.ok_or(AdminError::UnknownNode)?,
        };
        self.meshcore_add_contact(key, kind, &name)
            .await
            .map(|_| ())
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

    /// Marks a contact a favourite of the radio, or no longer, as the firmware
    /// keeps it: the lowest bit of its flags, the rest of them (who may ask
    /// it for telemetry) as they are. A favourite is never the one let go to
    /// make room. The contact is read from the radio and written back whole,
    /// as it is there now.
    pub(crate) async fn meshcore_favorite(
        &self,
        num: u32,
        favorite: bool,
    ) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        if self.read(|state| state.local.as_ref().map(|l| l.num)) == Some(num) {
            return Err(AdminError::OwnNode);
        }
        let key = self.meshcore_key(&session, num)?;
        let mut contact = match session
            .client
            .ask_one(command::get_contact(&key), |r| {
                matches!(r, Response::Contact(_))
            })
            .await
        {
            Ok(Response::Contact(contact)) => contact,
            Ok(Response::Error(_)) => return Err(AdminError::UnknownNode),
            Ok(_) => return Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        };
        contact.flags = (contact.flags & !1) | u8::from(favorite);
        contact.last_modified = now();
        session.client.done(command::add_contact(&contact)).await?;
        self.update(|state| match state.nodes.get_mut(&num) {
            Some(node) => {
                node.favorite = favorite;
                vec![Event::Node(node.clone())]
            }
            None => Vec::new(),
        });
        Ok(())
    }

    /// Has the radio forget a contact. Here it stays, as not in the radio's
    /// list.
    pub(crate) async fn meshcore_remove_contact(&self, num: u32) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        if self.read(|state| state.local.as_ref().map(|l| l.num)) == Some(num) {
            return Err(AdminError::OwnNode);
        }
        let key = self.meshcore_key(&session, num)?;
        forgotten(session.client.done(command::remove_contact(&key)).await)?;
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
        forgotten(session.client.done(command::reset_path(&key)).await)?;
        self.update(|state| match state.nodes.get_mut(&num) {
            Some(node) => {
                node.hops_away = None;
                vec![Event::Node(node.clone())]
            }
            None => Vec::new(),
        });
        Ok(())
    }

    /// Asks a sensor the radio logged in to what it kept of its readings
    /// over the last `seconds`: the lowest, the highest and the mean of each.
    /// The id it goes by, which its answer comes as a [`Event::Reply`] with.
    pub async fn meshcore_series(&self, num: u32, seconds: u32) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        // From so long ago until now, and two bytes the firmware keeps for
        // what it may take besides, none yet.
        let mut data = vec![request::SERIES];
        data.extend_from_slice(&seconds.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        self.send_asked(
            &session,
            num,
            Asking::Series(seconds),
            command::send_binary_request(&key, &data),
        )
        .await
    }

    /// Asks a contact the way to it and back, flooded: found, they come as
    /// an [`Event::Reply`] with the id it gives. The radio keeps the path it
    /// had.
    pub async fn meshcore_discover(&self, num: u32) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        self.send_asked(
            &session,
            num,
            Asking::Route,
            command::send_path_discovery(&key),
        )
        .await
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
        self.send_asked(
            &session,
            num,
            Asking::Anonymous(kind),
            command::send_anon_request(&key, &request),
        )
        .await
    }

    /// Asks a node the radio logged in to something, as the firmware numbers
    /// it in [`request`]: what a guest may ask, or an admin. The id it goes
    /// by, which its answer comes as a [`Event::Reply`] with; a node that
    /// does not take it, or one not logged in to, says nothing.
    pub async fn meshcore_request(&self, num: u32, what: u8) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let data = match what {
            // All the sensors it lets this login read: the mask the firmware
            // takes inverted, none masked.
            request::TELEMETRY => vec![what, 0, 0, 0],
            // As many as an answer holds, the strongest first, each by the
            // four bytes of its key a node number is made of, and four bytes
            // of chance to make the packet one of its own.
            request::NEIGHBOURS => {
                let chance = now().to_le_bytes();
                let mut data = vec![what, 0, NEIGHBOURS_ASKED, 0, 0, 2, 4];
                data.extend_from_slice(&chance);
                data
            }
            request::ACCESS_LIST => vec![what, 0, 0],
            _ => vec![what],
        };
        self.send_asked(
            &session,
            num,
            Asking::LoggedIn(what),
            command::send_binary_request(&key, &data),
        )
        .await
    }

    /// Sends a request once the radio waits for no other answer, and keeps
    /// it waiting for this one, as long as the radio expects it to take and
    /// at least [`ASK_WAIT`]: another would make it forget it. Given up on
    /// then, as [`Event::Unanswered`], if it has not been answered. Its tag.
    async fn send_asked(
        &self,
        session: &Session,
        num: u32,
        what: Asking,
        command: Vec<u8>,
    ) -> Result<u32, AdminError> {
        let waiting = Arc::clone(&session.waiting).lock_owned().await;
        let answer = session
            .client
            .ask_one(command, |r| matches!(r, Response::MessageSent { .. }))
            .await;
        let (tag, timeout_ms) = match answer {
            Ok(Response::MessageSent { tag, timeout_ms }) => (tag, timeout_ms),
            Ok(Response::Error(code)) => {
                return Err(AdminError::Refused(error_name(code).to_owned()));
            }
            Ok(_) => return Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        };
        let (answered, told) = tokio::sync::oneshot::channel();
        session
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                tag,
                Asked {
                    from: num,
                    what,
                    answered: Some(answered),
                },
            );
        let wait = Duration::from_millis(u64::from(timeout_ms) * 2).max(ASK_WAIT);
        tracing::debug!(num, ?what, timeout_ms, "a request sent");
        let (asked, mesh) = (Arc::clone(&session.asked), self.clone());
        tokio::spawn(async move {
            let timed_out = tokio::time::timeout(wait, told).await.is_err();
            drop(waiting);
            let lost = timed_out
                && asked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&tag)
                    .is_some();
            if lost {
                mesh.update(|_| {
                    vec![Event::Unanswered {
                        request_id: u32::from_le_bytes(tag),
                        from: num,
                    }]
                });
            }
        });
        Ok(u32::from_le_bytes(tag))
    }

    /// Runs a command on the radio's own console, as its CLI takes them: its
    /// answer, told as an [`Event::Console`] of its own node too.
    /// [`AdminError::Unsupported`] on firmware before 14, which has none.
    pub async fn meshcore_run(&self, command: &str) -> Result<String, AdminError> {
        let session = self.meshcore_session()?;
        let answer = session
            .client
            .ask_one(command::run_command(command), |r| {
                matches!(r, Response::CliReply(_))
            })
            .await;
        let text = match answer {
            Ok(Response::CliReply(text)) => text,
            Ok(Response::Error(1)) => return Err(AdminError::Unsupported),
            Ok(Response::Error(code)) => {
                return Err(AdminError::Refused(error_name(code).to_owned()));
            }
            Ok(_) => return Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        };
        let me = self.read(|state| state.local.as_ref().map(|l| l.num));
        if let Some(me) = me {
            let told = text.clone();
            self.update(|_| {
                vec![Event::Console {
                    from: me,
                    text: told,
                    at: now(),
                }]
            });
        }
        Ok(text)
    }

    /// Sends a command to the console of a node the radio logged in to as
    /// its admin: it answers, to its admin only, as an [`Event::Console`].
    /// Nothing says it arrived.
    pub async fn meshcore_command(&self, num: u32, command: &str) -> Result<(), AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let mut prefix = [0u8; 6];
        prefix.copy_from_slice(&key[..6]);
        let answer = session
            .client
            .ask_one(command::send_command(&prefix, command), |r| {
                matches!(r, Response::MessageSent { .. })
            })
            .await;
        match answer {
            Ok(Response::MessageSent { .. }) => Ok(()),
            Ok(Response::Error(code)) => Err(AdminError::Refused(error_name(code).to_owned())),
            Ok(_) => Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => Err(AdminError::NoAnswer),
            Err(_) => Err(AdminError::NotConnected),
        }
    }

    /// Pings a node the radio hears straight, a repeater's or a room's: a
    /// trace there and back, which needs no login. Its tag, which its answer
    /// comes as a [`Event::Reply`] with; [`AdminError::NoPath`] for one not
    /// heard straight.
    pub async fn meshcore_ping(&self, num: u32) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let direct = session
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&num)
            .is_some_and(|(path_len, _)| *path_len & 0x3f == 0 && *path_len != NO_PATH);
        if !direct {
            return Err(AdminError::NoPath);
        }
        // Its hash as the radio writes hashes: one, two or three bytes.
        let mode = self
            .read(|state| state.meshcore.as_ref().and_then(|s| s.path_hash_mode))
            .unwrap_or(0)
            .min(2);
        // A trace's hashes are 1, 2, 4 or 8 bytes, by the low two bits of
        // its flags: three-byte ones go as four.
        let size = 1usize << mode;
        self.send_trace(&session, num, mode, key[..size].to_vec(), None)
            .await
    }

    /// Traces the way to a repeater, a room or a sensor and back, along the
    /// path the radio keeps to it, of a hop at least: each hop on it passes
    /// it on with the SNR it heard it at. A room or a sensor passes it back
    /// only when set to repeat, which it is not out of the box. Its tag, which its answer comes as an
    /// [`Event::Traced`] with; [`AdminError::NoPath`] for one with no such
    /// path, [`AdminError::Unsupported`] for a node that passes nothing on.
    pub async fn meshcore_traceroute(&self, num: u32) -> Result<u32, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let passes_on = self.read(|state| {
            state
                .nodes
                .get(&num)
                .and_then(|n| n.user.as_ref())
                .is_some_and(|u| matches!(u.role.as_str(), "REPEATER" | "ROOM" | "SENSOR"))
        });
        if !passes_on {
            return Err(AdminError::Unsupported);
        }
        let (path_len, path) = session
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&num)
            .cloned()
            .ok_or(AdminError::UnknownNode)?;
        if path_len == NO_PATH || path_len & 0x3f == 0 {
            return Err(AdminError::NoPath);
        }
        // The path's hashes as a trace takes them, 1, 2, 4 or 8 bytes: a
        // three-byte one as its first two, as only three are known.
        let kept = usize::from(path_len >> 6) + 1;
        let mode = if kept >= 2 { 1 } else { 0 };
        let size = 1usize << mode;
        let relays: Vec<Vec<u8>> = path.chunks(kept).map(|h| h[..size].to_vec()).collect();
        // There through the relays to the node, and back the same way.
        let hops: Vec<Vec<u8>> = relays
            .iter()
            .cloned()
            .chain([key[..size].to_vec()])
            .chain(relays.iter().rev().cloned())
            .collect();
        self.send_trace(&session, num, mode, hops.concat(), Some(hops))
            .await
    }

    /// Sends a trace along a path of hashes, a ping's or a traceroute's: kept
    /// for its answer as long as the radio expects it to take twice, at
    /// least [`PING_WAIT`], given up on then as [`Event::Unanswered`].
    async fn send_trace(
        &self,
        session: &Session,
        num: u32,
        mode: u8,
        path: Vec<u8>,
        hops: Option<Vec<Vec<u8>>>,
    ) -> Result<u32, AdminError> {
        let tag = ping_tag(num);
        let answer = session
            .client
            .ask_one(command::send_trace_path(tag, mode, &path), |r| {
                matches!(r, Response::MessageSent { .. })
            })
            .await;
        let timeout_ms = match answer {
            Ok(Response::MessageSent { timeout_ms, .. }) => timeout_ms,
            Ok(Response::Error(code)) => {
                return Err(AdminError::Refused(error_name(code).to_owned()));
            }
            Ok(_) => return Err(AdminError::Refused("UNEXPECTED".to_owned())),
            Err("the radio did not answer") => return Err(AdminError::NoAnswer),
            Err(_) => return Err(AdminError::NotConnected),
        };
        let key = tag.to_le_bytes();
        session
            .pinged
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (num, std::time::Instant::now(), hops));
        // Given up on when twice as long as the radio expects has gone by,
        // and at least [`PING_WAIT`], if it has not come back by then.
        let wait = Duration::from_millis(u64::from(timeout_ms) * 2).max(PING_WAIT);
        let (pinged, mesh) = (Arc::clone(&session.pinged), self.clone());
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let lost = pinged
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key)
                .is_some();
            if lost {
                mesh.update(|_| {
                    vec![Event::Unanswered {
                        request_id: tag,
                        from: num,
                    }]
                });
            }
        });
        Ok(tag)
    }

    /// Sends a login to a room or a repeater, with a password, empty for a
    /// guest or one the node knows already: on the path the radio knows, or
    /// flooded. Its answer, to wait for with [`LoginSent::answer`], and how
    /// long to: twice what the radio expects, at least [`LOGIN_WAIT`].
    pub async fn meshcore_login(&self, num: u32, password: &str) -> Result<LoginSent, AdminError> {
        let session = self.meshcore_session()?;
        let key = self.meshcore_key(&session, num)?;
        let mut prefix = [0u8; 6];
        prefix.copy_from_slice(&key[..6]);
        // The radio waits for one answer at a time: this one, until it comes.
        let waiting = Arc::clone(&session.waiting).lock_owned().await;
        let (tell, told) = tokio::sync::oneshot::channel();
        session
            .logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(prefix, tell);
        let answer = session
            .client
            .ask_one(command::send_login(&key, password), |r| {
                matches!(r, Response::MessageSent { .. })
            })
            .await;
        let failed = match answer {
            Ok(Response::MessageSent { timeout_ms, .. }) => {
                return Ok(LoginSent {
                    wait: Duration::from_millis(u64::from(timeout_ms) * 2).max(LOGIN_WAIT),
                    told,
                    logins: Arc::clone(&session.logins),
                    _waiting: waiting,
                    prefix,
                });
            }
            Ok(Response::Error(code)) => AdminError::Refused(error_name(code).to_owned()),
            Ok(_) => AdminError::Refused("UNEXPECTED".to_owned()),
            Err("the radio did not answer") => AdminError::NoAnswer,
            Err(_) => AdminError::NotConnected,
        };
        session
            .logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&prefix);
        Err(failed)
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
        // MeshCore counts no hops as Meshtastic does.
        hop_limit: None,
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
        waiting: Arc::default(),
        pinged: Arc::default(),
        sending: Arc::default(),
        heard: Arc::default(),
        logins: Arc::default(),
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
    if let Err(why) = drain(mesh, &client, &session).await {
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
                    if let Err(why) = drain(mesh, &client, &session).await {
                        return why;
                    }
                }
                Some(Response::Advert | Response::PathUpdate) => {
                    contacts_due.get_or_insert(Instant::now() + CONTACTS_SETTLE);
                }
                // Heard advertising, but not kept by the radio: contacts
                // added by hand, or not of its kind, or too far, or the list
                // full. Known here, to be added from its page.
                Some(Response::NewContact(contact)) => {
                    remember_keys(&session, std::slice::from_ref(&contact));
                    let node = Node {
                        on_radio: Some(false),
                        ..node_from(&contact)
                    };
                    mesh.update(|state| {
                        state.nodes.insert(node.num, node.clone());
                        vec![Event::Node(node)]
                    });
                }
                // The radio full, and a new one in the place of another:
                // that one no longer on it, still known here.
                Some(Response::ContactDeleted(key)) => {
                    let num = num_of(&key);
                    mesh.update(|state| match state.nodes.get_mut(&num) {
                        Some(node) => {
                            node.on_radio = Some(false);
                            vec![Event::Node(node.clone())]
                        }
                        None => Vec::new(),
                    });
                }
                Some(Response::ContactsFull) => mesh.update(|_| vec![Event::ContactsFull]),
                Some(Response::Ack(tag)) => acknowledged(mesh, &session, tag),
                Some(Response::LoggedIn(LoggedIn { prefix, access, .. })) => {
                    logged_in(&session, prefix, Some(access));
                }
                Some(Response::NotLoggedIn(prefix)) => logged_in(&session, prefix, None),
                Some(Response::TraceData { tag, snrs, last_snr, .. }) => {
                    traced(mesh, &session, tag, &snrs, last_snr);
                }
                Some(Response::BinaryResponse { tag, data }) => {
                    replied(mesh, &session, tag, &data);
                }
                Some(Response::PathDiscovery {
                    prefix,
                    there,
                    back,
                }) => discovered(mesh, &session, prefix, &there, &back),
                Some(Response::RxLog { snr, rssi, raw }) => {
                    heard_raw(mesh, &session, snr, rssi, &raw);
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
async fn drain(mesh: &Mesh, client: &Client, session: &Session) -> Result<(), &'static str> {
    while let Some(message) = client.next_message().await? {
        match message {
            // A node's console answering: no message, out of the chat.
            Response::ContactMessage(message) if message.text_type == 1 => {
                let from = num_of(&message.prefix);
                mesh.update(|_| {
                    vec![Event::Console {
                        from,
                        text: message.text.clone(),
                        at: now(),
                    }]
                });
            }
            Response::ContactMessage(message) => {
                mesh.update(|state| direct(state, &message));
                let id = message_id(&message.prefix, message.timestamp, &message.text);
                matched_direct(mesh, session, message.prefix[0], id);
            }
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

/// A login sent, its answer still to come.
pub struct LoginSent {
    /// How long to wait for it.
    pub wait: Duration,
    told: tokio::sync::oneshot::Receiver<Option<Access>>,
    logins: Arc<Mutex<HashMap<[u8; 6], LoginWait>>>,
    prefix: [u8; 6],
    /// The radio held waiting for this answer, let go as it comes or not.
    _waiting: tokio::sync::OwnedMutexGuard<()>,
}

impl LoginSent {
    /// What the node let in as; refused as `WRONG_PASSWORD`, and with no
    /// answer in time as [`AdminError::NoAnswer`], as a node that does not
    /// take a password says nothing.
    pub async fn answer(self) -> Result<Access, AdminError> {
        match tokio::time::timeout(self.wait, self.told).await {
            Ok(Ok(Some(access))) => Ok(access),
            Ok(Ok(None)) => Err(AdminError::Refused("WRONG_PASSWORD".to_owned())),
            Ok(Err(_)) => Err(AdminError::NotConnected),
            Err(_) => {
                self.logins
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&self.prefix);
                Err(AdminError::NoAnswer)
            }
        }
    }
}

/// What the radio said to having something forgotten: done, also when it
/// says it had no such thing. A splitter in between that keeps contacts of
/// its own may list what the radio no longer has, and what is gone is as
/// forgotten as it was meant to be.
fn forgotten(said: Result<(), AdminError>) -> Result<(), AdminError> {
    match said {
        Err(AdminError::Refused(why)) if why == "NOT_FOUND" => Ok(()),
        said => said,
    }
}

/// A login answered: to the one waiting for it, if any still is.
fn logged_in(session: &Session, prefix: [u8; 6], access: Option<Access>) {
    let waiting = session
        .logins
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&prefix);
    if let Some(waiting) = waiting {
        let _ = waiting.send(access);
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
    let Some(Asked {
        from,
        what,
        answered,
    }) = asked
    else {
        return;
    };
    // The radio is free for the next request.
    if let Some(answered) = answered {
        let _ = answered.send(());
    }
    let room = mesh.read(|state| {
        state
            .nodes
            .get(&from)
            .and_then(|n| n.user.as_ref())
            .is_some_and(|u| u.role == "ROOM")
    });
    let Some(answer) = answer_of(what, data, room) else {
        tracing::debug!(from, ?what, "unreadable answer");
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

/// The way to a node and back, as a path discovery found them: told as
/// the answer to the one asked of the node whose key starts so.
fn discovered(mesh: &Mesh, session: &Session, prefix: [u8; 6], there: &Path, back: &Path) {
    let keys = session
        .keys
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let mut asked = session.asked.lock().unwrap_or_else(|e| e.into_inner());
    let tag = asked
        .iter()
        .find(|(_, a)| {
            a.what == Asking::Route && keys.get(&a.from).is_some_and(|k| k[..6] == prefix)
        })
        .map(|(tag, _)| *tag);
    let Some(Asked { from, answered, .. }) = tag.and_then(|tag| asked.remove(&tag)) else {
        tracing::debug!(?prefix, "a route found that none asked for");
        return;
    };
    drop(asked);
    // The radio is free for the next request.
    if let Some(answered) = answered {
        let _ = answered.send(());
    }
    let hops = |(len, hashes): &Path| {
        hashes
            .chunks(usize::from(len >> 6) + 1)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    mesh.update(|_| {
        vec![Event::Reply(crate::mesh::Reply {
            request_id: tag.map_or(0, u32::from_le_bytes),
            from,
            answer: crate::mesh::Answer::Route {
                there: hops(there),
                back: hops(back),
            },
        })]
    });
}

/// A ping's trace come back: how long it took, and how it was heard each
/// way.
fn traced(mesh: &Mesh, session: &Session, tag: [u8; 4], snrs: &[f32], last_snr: f32) {
    let pinged = session
        .pinged
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&tag);
    let Some((from, sent, hops)) = pinged else {
        return;
    };
    // A traceroute's: the hops it went along, and how each heard it.
    if let Some(hops) = hops {
        mesh.update(|_| {
            vec![Event::Traced {
                request_id: u32::from_le_bytes(tag),
                from,
                hops,
                snrs: snrs.to_vec(),
                last_snr,
            }]
        });
        return;
    }
    let round_trip_ms = u32::try_from(sent.elapsed().as_millis()).unwrap_or(u32::MAX);
    mesh.update(|_| {
        vec![Event::Reply(crate::mesh::Reply {
            request_id: u32::from_le_bytes(tag),
            from,
            answer: crate::mesh::Answer::Ping {
                round_trip_ms,
                snr_there: snrs.first().copied().unwrap_or(last_snr),
                snr_back: last_snr,
            },
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
        sent_at: Some(message.timestamp),
        author: message.author.map(u32::from_be_bytes),
        position: None,
        request_id: None,
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
        sent_at: Some(message.timestamp),
        author: None,
        position: None,
        request_id: None,
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
    fn what_is_not_there_is_as_forgotten() {
        assert_eq!(forgotten(Ok(())), Ok(()));
        assert_eq!(
            forgotten(Err(AdminError::Refused("NOT_FOUND".into()))),
            Ok(())
        );
        assert_eq!(
            forgotten(Err(AdminError::Refused("TABLE_FULL".into()))),
            Err(AdminError::Refused("TABLE_FULL".into()))
        );
        assert_eq!(
            forgotten(Err(AdminError::NoAnswer)),
            Err(AdminError::NoAnswer)
        );
    }

    #[test]
    fn regions_read_as_a_repeater_answers_them() {
        // As StaLocaia answered: its clock, then the names, padded.
        let mut data = 1_718_789_276u32.to_le_bytes().to_vec();
        data.extend(b"*,europe,#galicia,\0\0\0\0");
        assert_eq!(
            answer_of(Asking::Anonymous(anonymous::REGIONS), &data, false),
            Some(crate::mesh::Answer::Regions {
                names: vec!["europe".into(), "galicia".into()],
                unscoped: true,
                clock: Some(1_718_789_276),
            })
        );
        // None at all, and what was not asked.
        let none = 1u32.to_le_bytes();
        assert_eq!(
            answer_of(Asking::Anonymous(anonymous::REGIONS), &none, false),
            Some(crate::mesh::Answer::Regions {
                names: Vec::new(),
                unscoped: false,
                clock: Some(1),
            })
        );
        assert_eq!(answer_of(Asking::Anonymous(0x7f), &none, false), None);
        // Its owner, anonymously: after its clock, with no firmware.
        let mut anon = 1_700_000_000u32.to_le_bytes().to_vec();
        anon.extend(b"Torre\nAlguien");
        assert!(matches!(
            answer_of(Asking::Anonymous(anonymous::OWNER), &anon, false),
            Some(crate::mesh::Answer::Owner(o)) if o.name == "Torre" && o.owner == "Alguien" && o.firmware.is_empty()
        ));
        // Logged in: as asked, with nothing before.
        let owner = b"v1.18.0\nTorre\nAlguien";
        assert!(matches!(
            answer_of(Asking::LoggedIn(request::OWNER), owner, false),
            Some(crate::mesh::Answer::Owner(o)) if o.name == "Torre"
        ));
        assert_eq!(answer_of(Asking::LoggedIn(0x7f), owner, false), None);
        // Its access list: by the first six bytes of each key, and the bits.
        let listed = [
            0xc0, 0xc0, 0xc0, 0xc0, 1, 2, 3, 0x41, 0x48, 0x4f, 0x56, 5, 6, 0,
        ];
        assert_eq!(
            answer_of(Asking::LoggedIn(request::ACCESS_LIST), &listed, false),
            Some(crate::mesh::Answer::AccessList(vec![
                (0xc0c0_c0c0, 3),
                (0x4148_4f56, 0)
            ]))
        );
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
            author: None,
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
