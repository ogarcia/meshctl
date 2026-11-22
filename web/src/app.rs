// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface's frame, and what keeps it current: the connection to the
//! daemon, the radios, and what the selected one knows.
//!
//! The frame is two bars on the left and the page on the right. The first bar
//! holds the radios, one button each, and meshctl's own settings at its foot.
//! The second holds what there is to see of the selected radio: its pages, its
//! channels and its direct conversations, and the radio itself at its foot.
//! Over the page, a bar says what is showing and what can be done with it.

use crate::badge::{self, Face};
use crate::chat::{ChatView, peer_of, short_name, title};
use crate::daemon::{self, Events, Failure};
use crate::empty::EmptyState;
use crate::i18n::{I18nContextProvider, Locale, t, t_string, use_i18n};
use crate::map::{MapView, placed};
use crate::mesh::{MESSAGE_OFF, MeshView, STAR, ago, avatar_class, model, reach_words};
use crate::node::NodeView;
use crate::packets::{KEPT, PacketsView};
use crate::preferences::PreferencesView;
use crate::radios::{Editing, RadioEditor, RadiosView};
use crate::settings::SettingsView;
use gloo_timers::future::sleep;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::ServerMessage;
use meshctl_api::{
    Accent, Channel, Conversation, Language, LinkState, MapTheme, Message, NewRequest, Node,
    NodeCheck, NodeLatest, Pace, Packet, Peer, Preferences, Radio, RadioId, RadioKind, Request,
    RequestKind, RequestStatus, Status, Theme, Traceroute, TracerouteStatus,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

/// How long to wait before trying the daemon again.
const RETRY: Duration = Duration::from_secs(3);

/// How often "heard 5 min ago" is worked out again.
const CLOCK_TICK: Duration = Duration::from_secs(15);

/// How the connection to the daemon is doing, beyond the radios' own links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daemon {
    Connecting,
    Connected,
    Unreachable,
    NeedsToken,
}

/// Where a node's page was opened from, to go back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Back {
    Page(Page),
    /// Another node's page, as through its neighbours.
    Node(u32),
}

/// How many steps back are kept: enough for any walk through neighbours.
const BACK_KEPT: usize = 20;

/// Which page is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The selected radio's nodes where they are: the front page.
    Map,
    /// The selected radio's nodes.
    Nodes,
    /// What the selected radio hears, as it hears it.
    Packets,
    /// The open conversation.
    Chat,
    /// The open node.
    Node,
    /// The selected radio's own settings.
    RadioSettings,
    /// meshctl's: how the interface looks.
    Preferences,
    /// meshctl's: the radios it connects to.
    Radios,
    /// On a phone: the selected radio's channels and direct conversations,
    /// which a wider screen has aside.
    Chats,
    /// On a phone: the rest, that a wider screen has in its bars: the radio's
    /// own node and settings, the radios, the preferences.
    More,
}

impl Page {
    /// Whether it is one of meshctl's own settings, not a radio's.
    fn is_meshctl(self) -> bool {
        matches!(self, Self::Preferences | Self::Radios)
    }
}

/// Everything the interface shows, kept current by [`follow`].
#[derive(Clone, Copy)]
pub struct Live {
    pub daemon: RwSignal<Daemon>,
    pub page: RwSignal<Page>,
    /// Whether the bars are open over the page, on a narrow screen.
    pub menu: RwSignal<bool>,
    pub preferences: RwSignal<Preferences>,
    /// `None` until the daemon has said which radios there are.
    pub radios: RwSignal<Option<Vec<Radio>>>,
    pub selected: RwSignal<Option<RadioId>>,
    /// What the radios' editor is open on.
    pub radio_editing: RwSignal<Editing>,
    /// Of the selected radio.
    pub status: RwSignal<Option<Status>>,
    /// Of the selected radio.
    pub nodes: RwSignal<BTreeMap<u32, Node>>,
    /// The radio whose nodes are all read: until it is the selected one,
    /// what there is of it may be partial.
    pub loaded: RwSignal<Option<RadioId>>,
    /// Of the selected radio.
    pub channels: RwSignal<Vec<Channel>>,
    /// Of the selected radio.
    pub conversations: RwSignal<Vec<Conversation>>,
    /// The conversation open in the chat, if any.
    pub open: RwSignal<Option<Peer>>,
    /// The messages of the open conversation, oldest first.
    pub thread: RwSignal<Vec<Message>>,
    /// Of the selected radio, the newest first.
    pub traceroutes: RwSignal<Vec<Traceroute>>,
    /// What the selected radio heard, the newest first.
    pub packets: RwSignal<Vec<Packet>>,
    /// The node whose page is open, if any.
    pub node: RwSignal<Option<u32>>,
    /// Where the open node's page was opened from, the latest last.
    pub back: RwSignal<Vec<Back>>,
    /// The last page out of the settings, to go back to from them on a wider
    /// screen.
    pub came_from: RwSignal<Page>,
    /// Whether the open node's requests are shown, in their drawer.
    pub history: RwSignal<bool>,
    /// Whether the radios are shown to switch the selected one, on a phone.
    pub radio_sheet: RwSignal<bool>,
    /// The node picked on the map, if any.
    pub map_pick: RwSignal<Option<u32>>,
    /// A node for the map to go to once it is showing, as asked from its page.
    pub map_focus: RwSignal<Option<u32>>,
    /// The node whose last traced route the map shows, if any.
    pub map_route: RwSignal<Option<u32>>,
    /// What the open node was asked for, the newest first.
    pub requests: RwSignal<Vec<Request>>,
    /// What the open node last told of each kind.
    pub latest: RwSignal<Option<NodeLatest>>,
    /// What is amiss with the open node.
    pub checks: RwSignal<Vec<NodeCheck>>,
    /// Why the last request or traceroute was not sent.
    pub ask_error: RwSignal<Option<Failure>>,
    /// When, by this browser's clock, the selected radio sends each paced
    /// kind of thing again: until then it would drop it.
    pub ready: RwSignal<HashMap<Pace, u32>>,
    /// Bumped each time the selected radio syncs, for what is read only then.
    pub synced: RwSignal<u32>,
    /// Seconds since the epoch, ticking.
    pub now: RwSignal<u32>,
}

impl Live {
    fn new() -> Self {
        Self {
            daemon: RwSignal::new(Daemon::Connecting),
            page: RwSignal::new(Page::Map),
            menu: RwSignal::new(false),
            preferences: RwSignal::new(Preferences::default()),
            radios: RwSignal::new(None),
            selected: RwSignal::new(daemon::last_radio()),
            radio_editing: RwSignal::new(Editing::Closed),
            status: RwSignal::new(None),
            nodes: RwSignal::new(BTreeMap::new()),
            loaded: RwSignal::new(None),
            channels: RwSignal::new(Vec::new()),
            conversations: RwSignal::new(Vec::new()),
            open: RwSignal::new(None),
            thread: RwSignal::new(Vec::new()),
            traceroutes: RwSignal::new(Vec::new()),
            packets: RwSignal::new(Vec::new()),
            node: RwSignal::new(None),
            back: RwSignal::new(Vec::new()),
            came_from: RwSignal::new(Page::Map),
            history: RwSignal::new(false),
            radio_sheet: RwSignal::new(false),
            map_pick: RwSignal::new(None),
            map_focus: RwSignal::new(None),
            map_route: RwSignal::new(None),
            requests: RwSignal::new(Vec::new()),
            latest: RwSignal::new(None),
            checks: RwSignal::new(Vec::new()),
            ask_error: RwSignal::new(None),
            ready: RwSignal::new(HashMap::new()),
            synced: RwSignal::new(0),
            now: RwSignal::new(now()),
        }
    }

    /// Shows a page, and closes the bars over it on a narrow screen.
    pub fn go(self, page: Page) {
        let from = self.page.get_untracked();
        if crate::phone::back_of(from) != Some(Page::More) && from != Page::More {
            self.came_from.set(from);
        }
        self.page.set(page);
        self.menu.set(false);
        // A page gone to anew: nothing to go back to from it.
        self.back.set(Vec::new());
    }

    /// Shows a node on the map: picked, with its card, and the map gone to
    /// it.
    pub fn show_on_map(self, node: u32) {
        self.map_pick.set(Some(node));
        self.map_focus.set(Some(node));
        self.go(Page::Map);
    }

    /// Goes back to where the open node's page was opened from: another
    /// node's page, or a page; the nodes list when nothing is known.
    pub fn go_back(self) {
        let mut back = self.back.get_untracked();
        match back.pop().unwrap_or(Back::Page(Page::Nodes)) {
            Back::Node(node) => {
                self.show_node(node);
                self.back.set(back);
            }
            Back::Page(page) => self.go(page),
        }
    }

    pub fn select(self, radio: RadioId) {
        daemon::set_last_radio(radio);
        self.back.set(Vec::new());
        // What there is of the last one goes: nothing of it is shown as the
        // new one's while that is read.
        self.loaded.set(None);
        self.status.set(None);
        self.nodes.set(BTreeMap::new());
        self.selected.set(Some(radio));
        self.open.set(None);
        self.thread.set(Vec::new());
        self.node.set(None);
        self.requests.set(Vec::new());
        self.map_pick.set(None);
        self.map_route.set(None);
        if matches!(self.page.get_untracked(), Page::Chat | Page::Node)
            || self.page.get_untracked().is_meshctl()
        {
            self.page.set(Page::Map);
        }
        self.menu.set(false);
        spawn_local(async move {
            let _ = self.refresh_selected().await;
        });
    }

    /// Opens a conversation in the chat.
    pub fn open_chat(self, peer: Peer) {
        self.open.set(Some(peer));
        self.thread.set(Vec::new());
        self.go(Page::Chat);
        spawn_local(async move {
            let _ = self.refresh_thread().await;
        });
    }

    /// Changes the preferences here at once, and in the daemon for every
    /// other browser.
    pub fn prefer(self, preferences: Preferences) {
        self.preferences.set(preferences);
        spawn_local(async move {
            if let Ok(kept) = daemon::save_preferences(&preferences).await {
                self.preferences.set(kept);
            }
        });
    }

    /// Reads the open conversation's messages again.
    pub async fn refresh_thread(self) -> Result<(), Failure> {
        let (Some(radio), Some(peer)) = (self.selected.get_untracked(), self.open.get_untracked())
        else {
            return Ok(());
        };
        let messages = daemon::messages(radio, peer).await?;
        if self.selected.get_untracked() == Some(radio) && self.open.get_untracked() == Some(peer) {
            self.thread.set(messages);
        }
        Ok(())
    }

    async fn refresh_conversations(self) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            return Ok(());
        };
        let conversations = daemon::conversations(radio).await?;
        if self.selected.get_untracked() == Some(radio) {
            self.conversations.set(conversations);
        }
        Ok(())
    }

    /// Reads the radios again, and keeps a selection that still exists.
    pub async fn refresh_radios(self) -> Result<(), Failure> {
        let radios = daemon::radios().await?;
        let selected = self.selected.get_untracked();
        if !selected.is_some_and(|id| radios.iter().any(|r| r.id == id)) {
            // The first on the rail, if any is.
            let first = radios
                .iter()
                .find(|r| r.shown)
                .or(radios.first())
                .map(|r| r.id);
            self.selected.set(first);
        }
        self.radios.set(Some(radios));
        Ok(())
    }

    /// Reads what the selected radio knows again.
    pub async fn refresh_selected(self) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            self.loaded.set(None);
            self.status.set(None);
            self.nodes.set(BTreeMap::new());
            self.channels.set(Vec::new());
            self.conversations.set(Vec::new());
            return Ok(());
        };
        let status = daemon::status(radio).await?;
        let nodes = daemon::nodes(radio).await?;
        let channels = daemon::channels(radio).await?;
        let conversations = daemon::conversations(radio).await?;
        let traceroutes = daemon::traceroutes(radio).await?;
        let packets = daemon::packets(radio, None).await?;
        // The selection may have moved on while we were asking.
        if self.selected.get_untracked() == Some(radio) {
            self.status.set(Some(status));
            self.nodes
                .set(nodes.into_iter().map(|node| (node.num, node)).collect());
            self.channels.set(channels);
            self.conversations.set(conversations);
            self.traceroutes.set(traceroutes);
            self.packets.set(packets);
            self.loaded.set(Some(radio));
        }
        self.refresh_thread().await
    }

    /// Opens a node's page.
    pub fn open_node(self, node: u32) {
        // Where it is opened from, to go back to; the same page again is
        // nowhere new.
        let from = match self.page.get_untracked() {
            Page::Node => self.node.get_untracked().map(Back::Node),
            page => Some(Back::Page(page)),
        };
        let mut back = self.back.get_untracked();
        if from != Some(Back::Node(node)) {
            back.extend(from);
        }
        if back.len() > BACK_KEPT {
            back.remove(0);
        }
        self.show_node(node);
        self.back.set(back);
    }

    /// Shows a node's page, and reads what it shows.
    fn show_node(self, node: u32) {
        self.node.set(Some(node));
        self.history.set(false);
        self.requests.set(Vec::new());
        self.latest.set(None);
        self.checks.set(Vec::new());
        self.ask_error.set(None);
        self.go(Page::Node);
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(requests) = daemon::requests(radio, node).await
                && self.node.get_untracked() == Some(node)
                && self.selected.get_untracked() == Some(radio)
            {
                self.requests.set(requests);
            }
        });
        self.read_waits(radio);
        self.read_latest(radio, node);
        self.read_checks(radio, node);
    }

    /// Reads what the open node last told of each kind.
    fn read_latest(self, radio: RadioId, node: u32) {
        spawn_local(async move {
            if let Ok(latest) = daemon::node_latest(radio, node).await
                && self.node.get_untracked() == Some(node)
                && self.selected.get_untracked() == Some(radio)
            {
                self.latest.set(Some(latest));
            }
        });
    }

    /// Reads what is amiss with the open node.
    fn read_checks(self, radio: RadioId, node: u32) {
        spawn_local(async move {
            if let Ok(checks) = daemon::node_checks(radio, node).await
                && self.node.get_untracked() == Some(node)
                && self.selected.get_untracked() == Some(radio)
            {
                self.checks.set(checks);
            }
        });
    }

    /// Reads how long the radio takes to send each paced kind again.
    fn read_waits(self, radio: RadioId) {
        spawn_local(async move {
            if let Ok(waits) = daemon::waits(radio).await
                && self.selected.get_untracked() == Some(radio)
            {
                let at = now();
                self.ready.set(
                    [Pace::Position, Pace::Telemetry, Pace::Traceroute]
                        .into_iter()
                        .map(|pace| {
                            let wait = u32::try_from(waits.of(pace)).unwrap_or_default();
                            (pace, at + wait)
                        })
                        .collect(),
                );
            }
        });
    }

    /// Something of a pace just sent through the selected radio, here or
    /// from another client: the like waits its interval from now.
    fn paced(self, pace: Pace) {
        let until = now() + u32::try_from(pace.interval()).unwrap_or_default();
        self.ready.update(|ready| {
            let at = ready.entry(pace).or_default();
            *at = (*at).max(until);
        });
    }

    /// How many seconds until the selected radio sends something of a pace
    /// again, by a clock that ticks as often as needed.
    pub fn wait(self, pace: Pace, now: u32) -> u32 {
        self.ready
            .with(|ready| ready.get(&pace).map_or(0, |at| at.saturating_sub(now)))
    }

    /// Asks a node for something, on a channel of the selected radio.
    pub fn ask(self, node: u32, kind: RequestKind, channel: u32) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        self.ask_error.set(None);
        spawn_local(async move {
            let new = NewRequest {
                node,
                kind,
                channel,
            };
            match daemon::ask(radio, &new).await {
                Ok(request) => {
                    if let Some(pace) = kind.pace() {
                        self.paced(pace);
                    }
                    self.keep_request(request);
                }
                Err(failure) => self.refused(radio, failure),
            }
        });
    }

    /// Puts a request of the open node in its list, or where it was.
    fn keep_request(self, request: Request) {
        if self.node.get_untracked() != Some(request.node) {
            return;
        }
        self.requests.update(
            |requests| match requests.iter_mut().find(|r| r.id == request.id) {
                Some(kept) => *kept = request,
                None => requests.insert(0, request),
            },
        );
    }

    /// Traces the route to a node from the selected radio, on a channel.
    pub fn trace(self, node: u32, channel: u32) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        self.ask_error.set(None);
        spawn_local(async move {
            match daemon::trace(radio, node, channel).await {
                Ok(traceroute) => {
                    self.paced(Pace::Traceroute);
                    self.keep_traceroute(traceroute);
                }
                Err(failure) => self.refused(radio, failure),
            }
        });
    }

    /// A request or traceroute not sent, and why. Too soon, the waits are
    /// read again: another client may have sent the like.
    fn refused(self, radio: RadioId, failure: Failure) {
        if failure == Failure::Rejected(meshctl_api::ErrorCode::TooSoon) {
            self.read_waits(radio);
        }
        self.ask_error.set(Some(failure));
    }

    /// Forgets what the open node was asked for, but what still waits: here
    /// as in the daemon.
    pub async fn forget_requests(self, node: u32) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            return Ok(());
        };
        daemon::forget_requests(radio, node).await?;
        self.requests.update(|requests| {
            requests.retain(|r| r.node != node || r.status == RequestStatus::Pending);
        });
        Ok(())
    }

    /// Forgets the routes traced to a node but the latest and those waiting:
    /// here as in the daemon.
    pub async fn forget_traceroutes(self, node: u32) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            return Ok(());
        };
        daemon::forget_traceroutes(radio, node).await?;
        self.traceroutes.update(|traceroutes| {
            let latest = traceroutes
                .iter()
                .filter(|t| t.target == node)
                .map(|t| t.id)
                .max();
            traceroutes.retain(|t| {
                t.target != node || Some(t.id) == latest || t.status == TracerouteStatus::Pending
            });
        });
        Ok(())
    }

    /// Puts a traceroute in the list, or where it was, as it is now.
    fn keep_traceroute(self, traceroute: Traceroute) {
        self.traceroutes.update(|traceroutes| {
            match traceroutes.iter_mut().find(|t| t.id == traceroute.id) {
                Some(kept) => *kept = traceroute,
                None => traceroutes.insert(0, traceroute),
            }
        });
    }

    pub async fn refresh(self) -> Result<(), Failure> {
        self.preferences.set(daemon::preferences().await?);
        self.refresh_radios().await?;
        self.refresh_selected().await
    }

    /// The selected radio, as the daemon last said.
    pub fn selected_radio(self) -> Option<Radio> {
        let id = self.selected.get()?;
        self.radios
            .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).cloned())
    }

    /// Which mesh the selected radio speaks.
    pub fn selected_kind(self) -> Option<RadioKind> {
        let id = self.selected.get()?;
        self.radios
            .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).map(|r| r.kind))
    }

    fn is_selected(self, radio: RadioId) -> bool {
        self.selected.get_untracked() == Some(radio)
    }

    /// A channel's name, as the radio has it, if it has the channel.
    fn channel_name(self, channel: u32) -> Option<String> {
        self.channels.with_untracked(|channels| {
            channels
                .iter()
                .find(|c| c.index == channel)
                .map(|c| c.name.clone())
        })
    }

    /// With the daemon out of reach nothing is known of the radios' links:
    /// none is taken as up, so nothing is offered that needs one, until a
    /// new session reads them again.
    /// A radio switched off stays so: that is meshctl's own setting, known
    /// without asking.
    fn lose_links(self) {
        let lost = |link: &mut LinkState| {
            if *link != LinkState::Disabled {
                *link = LinkState::Disconnected;
            }
        };
        self.radios.update(|radios| {
            for radio in radios.iter_mut().flatten() {
                lost(&mut radio.link);
            }
        });
        self.status.update(|status| {
            if let Some(status) = status {
                lost(&mut status.link);
            }
        });
    }

    /// The class for a radio's link, as far as is known: with the daemon out
    /// of reach, that, but for a radio switched off.
    pub fn link_class(self, link: LinkState) -> &'static str {
        if self.daemon.get() == Daemon::Connected || link == LinkState::Disabled {
            link_class(link)
        } else {
            "daemon-down"
        }
    }

    /// One connection to the event stream, until it drops.
    async fn session(self) -> Failure {
        let mut events = match Events::open().await {
            Ok(events) => events,
            Err(failure) => return failure,
        };
        // After ready, not before: nothing is lost in between.
        if let Err(failure) = self.refresh().await {
            return failure;
        }
        self.daemon.set(Daemon::Connected);

        while let Some(message) = events.next().await {
            let refreshed = match message {
                ServerMessage::Link { radio, link } => {
                    self.radios.update(|radios| {
                        if let Some(r) = radios.iter_mut().flatten().find(|r| r.id == radio) {
                            r.link = link;
                        }
                    });
                    if self.is_selected(radio) {
                        self.status.update(|status| {
                            if let Some(status) = status {
                                status.link = link;
                            }
                        });
                    }
                    // In sync: what it said of itself, its short name, is
                    // read again.
                    if link == LinkState::Ready {
                        self.refresh_radios().await
                    } else {
                        Ok(())
                    }
                }
                ServerMessage::Node { radio, node } => {
                    if self.is_selected(radio) {
                        self.nodes.update(|nodes| {
                            nodes.insert(node.num, node);
                        });
                    }
                    Ok(())
                }
                ServerMessage::Synced { radio } if self.is_selected(radio) => {
                    self.synced.update(|n| *n += 1);
                    self.refresh_selected().await
                }
                ServerMessage::Message { radio, message } if self.is_selected(radio) => {
                    let peer = peer_of(&message);
                    let open = self.open.get_untracked();
                    if open == Some(peer) {
                        self.thread.update(|thread| {
                            if !thread.iter().any(|m| m.id == message.id) {
                                thread.push(message);
                            }
                        });
                    } else if let (
                        Some(Peer::Channel { channel: open }),
                        Peer::Channel { channel: heard },
                    ) = (open, peer)
                        && self.channel_name(open) == self.channel_name(heard)
                    {
                        // The very same channel in two slots is one
                        // conversation, which says the first slot: the one
                        // open may be the other. The very same is named the
                        // same; two only named the same cost a reading more.
                        if let Err(failure) = self.refresh_thread().await {
                            return failure;
                        }
                    }
                    self.refresh_conversations().await
                }
                ServerMessage::MessageStatus {
                    radio,
                    id,
                    status,
                    error,
                } if self.is_selected(radio) => {
                    self.thread.update(|thread| {
                        if let Some(message) = thread.iter_mut().find(|m| m.id == id) {
                            message.status = status;
                            message.error = error;
                        }
                    });
                    Ok(())
                }
                ServerMessage::Traceroute { radio, traceroute } if self.is_selected(radio) => {
                    if traceroute.status == TracerouteStatus::Pending {
                        self.paced(Pace::Traceroute);
                    }
                    self.keep_traceroute(traceroute);
                    Ok(())
                }
                ServerMessage::Request { radio, request } if self.is_selected(radio) => {
                    if request.status == RequestStatus::Pending
                        && let Some(pace) = request.kind.pace()
                    {
                        self.paced(pace);
                    }
                    self.keep_request(request);
                    Ok(())
                }
                ServerMessage::Packet { radio, packet } if self.is_selected(radio) => {
                    // Something new from the open node that its page shows.
                    if self.node.get_untracked() == Some(packet.from) {
                        match packet.port.as_deref() {
                            Some("NODEINFO_APP") => {
                                self.read_latest(radio, packet.from);
                                self.read_checks(radio, packet.from);
                            }
                            Some("POSITION_APP" | "TELEMETRY_APP" | "NEIGHBORINFO_APP") => {
                                self.read_latest(radio, packet.from);
                            }
                            _ => {}
                        }
                    }
                    self.packets.update(|packets| {
                        packets.insert(0, packet);
                        packets.truncate(KEPT);
                    });
                    Ok(())
                }
                ServerMessage::Preferences { preferences } => {
                    self.preferences.set(preferences);
                    Ok(())
                }
                ServerMessage::Radios | ServerMessage::Lagged => self.refresh().await,
                _ => Ok(()),
            };
            if let Err(failure) = refreshed {
                return failure;
            }
        }
        Failure::Unreachable
    }
}

/// Follows the daemon for as long as the page is open.
fn follow(live: Live) {
    spawn_local(async move {
        loop {
            match live.session().await {
                Failure::Unauthorized => {
                    live.daemon.set(Daemon::NeedsToken);
                    // Until a token is given.
                    while live.daemon.get_untracked() == Daemon::NeedsToken {
                        sleep(Duration::from_millis(300)).await;
                    }
                }
                Failure::Unreachable | Failure::Rejected(_) => {
                    live.daemon.set(Daemon::Unreachable);
                    live.lose_links();
                    sleep(RETRY).await;
                }
            }
            live.daemon.set(Daemon::Connecting);
        }
    });
    spawn_local(async move {
        loop {
            sleep(CLOCK_TICK).await;
            live.now.set(now());
        }
    });
}

pub fn now() -> u32 {
    // Milliseconds as an f64, which holds them exactly for a long while yet.
    (js_sys::Date::now() / 1000.0) as u32
}

/// Puts the preferences on `<html>`, where the style sheet reads them, as the
/// daemon already did when it served the page.
fn show(preferences: Preferences) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    let set = |name: &str, value: Option<&str>| {
        let _ = match value {
            Some(value) => root.set_attribute(name, value),
            None => root.remove_attribute(name),
        };
    };
    set(
        "data-theme",
        match preferences.theme {
            Theme::System => None,
            Theme::Light => Some("light"),
            Theme::Dark => Some("dark"),
        },
    );
    set(
        "data-accent",
        Some(match preferences.accent {
            Accent::Amber => "amber",
        }),
    );
    set(
        "data-map-theme",
        match preferences.map_theme {
            MapTheme::Page => None,
            MapTheme::Light => Some("light"),
            MapTheme::Dark => Some("dark"),
        },
    );
}

#[component]
pub fn App() -> impl IntoView {
    view! {
        // The language is a preference meshctl keeps, for every browser: not a
        // cookie of this one's, which would outlive it being changed.
        <I18nContextProvider enable_cookie=false>
            <Shell />
        </I18nContextProvider>
    }
}

/// The first of the browser's languages the interface speaks, or English.
fn browser_locale() -> Locale {
    let languages = web_sys::window()
        .map(|w| w.navigator().languages())
        .map(|list| {
            list.iter()
                .filter_map(|l| l.as_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    languages
        .iter()
        .find_map(|language| {
            let language = language.to_lowercase();
            if language.starts_with("es") {
                Some(Locale::es)
            } else if language.starts_with("en") {
                Some(Locale::en)
            } else {
                None
            }
        })
        .unwrap_or(Locale::en)
}

#[component]
fn Shell() -> impl IntoView {
    let i18n = use_i18n();
    let live = Live::new();
    provide_context(live);
    follow(live);

    Effect::new(move |_| {
        let preferences = live.preferences.get();
        show(preferences);
        i18n.set_locale(match preferences.language {
            Some(Language::En) => Locale::en,
            Some(Language::Es) => Locale::es,
            None => browser_locale(),
        });
    });

    let class = move || {
        if live.menu.get() {
            "app menu-open"
        } else {
            "app"
        }
    };
    view! {
        // On a wide screen the map takes the whole page, without the bar over
        // it: what that would say, its nodes' cards say. The style sheet
        // decides, by the width.
        // A phone has a bar of tabs at the foot instead of the two aside, but
        // in a conversation, which takes the whole screen to write.
        <div
            class=class
            class:on-map=move || live.page.get() == Page::Map
            class:in-chat=move || live.page.get() == Page::Chat
        >
            <Rail />
            <Sidebar />
            // Over the page while the bars are open on a narrow screen: a touch
            // anywhere off them closes them.
            <button
                class="menu-backdrop"
                tabindex="-1"
                aria-hidden="true"
                on:click=move |_| live.menu.set(false)
            ></button>
            <div class="page">
                <Topbar />
                <main>
                    <Content />
                </main>
            </div>
            <crate::phone::TabBar />
            <crate::phone::RadioSheet />
        </div>
    }
}

/// What the page shows.
#[component]
fn Content() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // What to show, worked out apart: the daemon going up and down, or the
    // radios being read again, leave the page as it is unless it changes.
    // Otherwise the map would be built anew on every retry.
    let shown = Memo::new(move |_| {
        match (
            live.daemon.get(),
            live.radios.with(|r| r.as_ref().map(Vec::len)),
            live.page.get(),
        ) {
            (Daemon::NeedsToken, _, _) => Shown::Token,
            (Daemon::Unreachable, None, _) => Shown::Down,
            (_, None, _) => Shown::Nothing,
            (_, _, Page::Preferences) => Shown::Page(Page::Preferences),
            (_, _, Page::Radios) => Shown::Page(Page::Radios),
            (_, Some(0), _) => Shown::Welcome,
            (_, Some(_), page) => Shown::Page(page),
        }
    });
    move || match shown.get() {
        Shown::Token => view! { <TokenForm /> }.into_any(),
        Shown::Down => view! {
            <EmptyState
                icon=crate::empty::SERVER_OFF
                title=t!(i18n, empty_daemon_title).into_any()
                text=t!(i18n, empty_daemon_text).into_any()
            />
        }
        .into_any(),
        Shown::Nothing => ().into_any(),
        Shown::Welcome => view! { <Welcome /> }.into_any(),
        Shown::Page(Page::Preferences) => view! { <PreferencesView /> }.into_any(),
        Shown::Page(Page::Radios) => view! { <RadiosView /> }.into_any(),
        Shown::Page(Page::Map) => view! { <MapView /> }.into_any(),
        Shown::Page(Page::Nodes) => view! { <MeshView /> }.into_any(),
        Shown::Page(Page::Chat) => view! { <ChatView /> }.into_any(),
        Shown::Page(Page::Node) => view! { <NodeView /> }.into_any(),
        Shown::Page(Page::Packets) => view! { <PacketsView /> }.into_any(),
        Shown::Page(Page::RadioSettings) => view! { <SettingsView /> }.into_any(),
        Shown::Page(Page::Chats) => view! { <crate::phone::ChatsView /> }.into_any(),
        Shown::Page(Page::More) => view! { <crate::phone::MoreView /> }.into_any(),
    }
}

/// What [`Content`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Token,
    Down,
    Nothing,
    Welcome,
    Page(Page),
}

const GEAR: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/settings.svg");
const MESSAGE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/message.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");

/// The first bar: the radios, and meshctl's own settings.
#[component]
fn Rail() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // Those to be shown, in their order.
    let radios = move || {
        let radios = live.radios.get().unwrap_or_default();
        radios.into_iter().filter(|r| r.shown).collect::<Vec<_>>()
    };
    let settings_class = move || {
        if live.page.get().is_meshctl() {
            "rail-button active"
        } else {
            "rail-button"
        }
    };
    view! {
        <nav class="rail" aria-label=move || t_string!(i18n, nav_radios)>
            <svg class="logo" viewBox="0 0 24 24" role="img" aria-label="meshctl">
                <circle cx="5" cy="12" r="2.5"></circle>
                <circle cx="19" cy="5" r="2.5"></circle>
                <circle cx="19" cy="19" r="2.5"></circle>
                <path d="M7.3 11 16.7 6M7.3 13l9.4 5"></path>
            </svg>
            <span class="rail-rule"></span>
            <For
                each=radios
                key=|radio| {
                    (radio.id, radio.name.clone(), radio.badge.clone(), radio.short_name.clone())
                }
                let:radio
            >
                <RadioButton
                    id=radio.id
                    name=radio.name.clone()
                    badge=radio.badge.clone()
                    short=radio.short_name.clone()
                />
            </For>
            <span class="rail-space"></span>
            <button
                class=settings_class
                title=move || t_string!(i18n, nav_meshctl_settings)
                aria-label=move || t_string!(i18n, nav_meshctl_settings)
                // meshctl's settings open on the radios, the most of them.
                on:click=move |_| {
                    if !live.page.get_untracked().is_meshctl() {
                        live.radio_editing.set(Editing::Closed);
                        live.go(Page::Radios);
                    }
                }
                inner_html=GEAR
            ></button>
        </nav>
    }
}

#[component]
fn RadioButton(
    id: RadioId,
    name: String,
    badge: meshctl_api::Badge,
    short: Option<String>,
) -> impl IntoView {
    let live = expect_context::<Live>();
    let link = move || {
        live.radios.with(|radios| {
            radios
                .iter()
                .flatten()
                .find(|r| r.id == id)
                .map_or(LinkState::Disconnected, |r| r.link)
        })
    };
    let colored = if badge.color.is_some() {
        " colored"
    } else {
        ""
    };
    let class = move || {
        let active = live.selected.get() == Some(id) && !live.page.get().is_meshctl();
        let active = if active { " active" } else { "" };
        format!("rail-button radio{colored}{active}")
    };
    let style = badge::style(&badge);
    view! {
        <button
            class=class
            style=style
            title=name.clone()
            aria-label=name.clone()
            on:click=move |_| live.select(id)
        >
            <Face name=name.clone() badge=badge.clone() short=short.clone() />
            <span class=move || format!("dot {}", live.link_class(link()))></span>
        </button>
    }
}

/// The second bar: what there is of the selected radio, or of meshctl's own
/// settings.
#[component]
fn Sidebar() -> impl IntoView {
    let live = expect_context::<Live>();
    view! {
        <aside class="sidebar">
            {move || {
                if live.page.get().is_meshctl() {
                    view! { <MeshctlSidebar /> }.into_any()
                } else {
                    view! { <RadioSidebar /> }.into_any()
                }
            }}
        </aside>
    }
}

#[component]
fn MeshctlSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let item = move |page: Page| {
        let class = move || {
            if live.page.get() == page {
                "item active"
            } else {
                "item"
            }
        };
        let label = move || match page {
            Page::Preferences => t!(i18n, prefs_title).into_any(),
            _ => t!(i18n, radios_title).into_any(),
        };
        // The radios' page opens on their list, the editor closed.
        let open = move |_| {
            if page == Page::Radios && live.page.get_untracked() != page {
                live.radio_editing.set(Editing::Closed);
            }
            live.go(page);
        };
        view! { <button class=class on:click=open>{label}</button> }
    };
    view! {
        <header class="sidebar-head">
            <h1>"meshctl"</h1>
            <span class="dim">{t!(i18n, nav_meshctl_settings)}</span>
        </header>
        <div class="sidebar-body">{item(Page::Radios)} {item(Page::Preferences)}</div>
    }
}

#[component]
fn RadioSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let page_item = move |page: Page| {
        let class = move || {
            if live.page.get() == page {
                "item active"
            } else {
                "item"
            }
        };
        let (label, count) = match page {
            Page::Map => (
                t!(i18n, page_map).into_any(),
                Some(Signal::derive(move || placed(live))),
            ),
            Page::Nodes => (
                t!(i18n, nodes_title).into_any(),
                Some(Signal::derive(move || live.nodes.with(BTreeMap::len))),
            ),
            _ => (t!(i18n, page_packets).into_any(), None),
        };
        view! {
            <button class=class on:click=move |_| live.go(page)>
                {label}
                {count.map(|count| view! { <span class="count">{count}</span> })}
            </button>
        }
    };
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| Peer::Channel { channel: c.index })
                .collect::<Vec<_>>()
        })
    };
    let directs = move || {
        live.conversations.with(|conversations| {
            conversations
                .iter()
                .map(|c| c.peer)
                .filter(|peer| matches!(peer, Peer::Node { .. }))
                .collect::<Vec<_>>()
        })
    };

    view! {
        <Show when=move || live.selected.get().is_some()>
            <RadioHead />
            <div class="sidebar-body">
                {page_item(Page::Map)}
                {page_item(Page::Nodes)}
                {page_item(Page::Packets)}
                <h2>{t!(i18n, nav_channels)}</h2>
                <For each=channels key=|peer| *peer let:peer>
                    <PeerItem peer />
                </For>
                <h2>{t!(i18n, nav_directs)}</h2>
                <For each=directs key=|peer| *peer let:peer>
                    <PeerItem peer />
                </For>
                <Show when=move || directs().is_empty()>
                    <p class="empty dim">{t!(i18n, nav_no_directs)}</p>
                </Show>
            </div>
            <LocalFoot />
        </Show>
    }
}

/// The selected radio's name, and how it is.
#[component]
fn RadioHead() -> impl IntoView {
    let live = expect_context::<Live>();
    let radio = move || live.selected_radio();
    let link = Signal::derive(move || radio().map_or(LinkState::Connecting, |r| r.link));
    let kind = move || match radio().map(|r| r.kind) {
        Some(RadioKind::Meshtastic) => "Meshtastic",
        Some(RadioKind::Meshcore) => "MeshCore",
        None => "",
    };
    view! {
        <header class="sidebar-head">
            <h1>{move || radio().map(|r| r.name)}</h1>
            // In one line: if it does not fit, the link gives way. How many
            // nodes there are is said by the list below.
            <span class="dim facts">
                <span>{kind}</span>
                <span class="link">
                    <LinkLabel link />
                </span>
            </span>
        </header>
    }
}

/// A channel or a node to talk to.
#[component]
pub(crate) fn PeerItem(peer: Peer) -> impl IntoView {
    let live = expect_context::<Live>();
    let class = move || {
        if live.page.get() == Page::Chat && live.open.get() == Some(peer) {
            "item active"
        } else {
            "item"
        }
    };
    let marker = match peer {
        Peer::Channel { .. } => view! { <span class="mark-channel">"#"</span> }.into_any(),
        Peer::Node { node } => {
            let hops = move || live.nodes.with(|n| n.get(&node).and_then(|n| n.hops_away));
            view! {
                <span class=move || match hops() {
                    Some(0) => "mark-node direct",
                    Some(1..=2) => "mark-node near",
                    Some(3..=4) => "mark-node mid",
                    _ => "mark-node far",
                }></span>
            }
            .into_any()
        }
    };
    let label = move || {
        let title = title(live, peer);
        // The sidebar marks channels already.
        title
            .strip_prefix("# ")
            .map_or_else(|| title.clone(), str::to_owned)
    };
    view! {
        <button class=class on:click=move |_| live.open_chat(peer)>
            {marker}
            <span class="label">{label}</span>
        </button>
    }
}

/// The selected radio's own node, and the way to its settings.
#[component]
pub(crate) fn LocalFoot() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let local = move || live.status.get().and_then(|s| s.local);
    let num = move || local().map(|l| l.num);
    let name = move || {
        let num = num()?;
        live.nodes.with(|nodes| nodes.get(&num)?.long_name.clone())
    };
    // Its node's short name, or, with none known yet, what its button on the
    // rail shows.
    let short = move || {
        num()
            .map(|num| short_name(live, num))
            .filter(|short| !short.is_empty())
            .or_else(|| {
                live.selected_radio()
                    .map(|r| r.short_name.unwrap_or_else(|| badge::initials(&r.name)))
            })
            .unwrap_or_default()
    };
    let detail = move || {
        local().map(|l| match l.hw_model.as_deref().map(model) {
            Some(hw) => format!("{} · {hw}", l.id),
            None => l.id,
        })
    };
    let class = move || {
        if live.page.get() == Page::RadioSettings {
            "settings active"
        } else {
            "settings"
        }
    };
    // In the radio's own colour, if it was given one, as its button on the
    // rail is when selected.
    let badge = move || live.selected_radio().map(|r| r.badge).unwrap_or_default();
    let avatar_class = move || {
        if badge().color.is_some() {
            "avatar colored"
        } else {
            "avatar"
        }
    };
    view! {
        <footer class="local-foot">
            <span class=avatar_class style=move || badge::style(&badge())>
                {short}
            </span>
            <span class="who">
                <b>{move || name().unwrap_or_else(|| t_string!(i18n, nav_my_node).to_owned())}</b>
                <span class="mono dim">{detail}</span>
            </span>
            <button
                class=class
                title=move || t_string!(i18n, nav_radio_settings)
                aria-label=move || t_string!(i18n, nav_radio_settings)
                on:click=move |_| live.go(Page::RadioSettings)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12"></path>
                    <circle cx="16" cy="6" r="2"></circle>
                    <circle cx="10" cy="12" r="2"></circle>
                    <circle cx="18" cy="18" r="2"></circle>
                </svg>
            </button>
        </footer>
    }
}

/// Over the page: what is showing, and what can be done with it.
#[component]
fn Topbar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let heading = move || match live.page.get() {
        // A tab's own page, on a phone, says the radio instead of its name,
        // which the bar of tabs says.
        Page::Nodes => view! { <h1 class="page-title">{t!(i18n, nodes_title)}</h1> }.into_any(),
        Page::Map => match live.map_pick.get() {
            // On a phone the node's card says it all, under the radio.
            Some(node) => view! {
                <div class="wide-only">
                    <PeerHeading peer=Peer::Node { node } />
                </div>
            }
            .into_any(),
            None => view! { <h1 class="page-title">{t!(i18n, page_map)}</h1> }.into_any(),
        },
        Page::Packets => view! { <h1 class="page-title">{t!(i18n, page_packets)}</h1> }.into_any(),
        Page::Chats => view! { <h1 class="page-title">{t!(i18n, page_chats)}</h1> }.into_any(),
        Page::More => view! { <h1 class="page-title">{t!(i18n, page_more)}</h1> }.into_any(),
        Page::Node => match live.node.get() {
            Some(node) => view! { <PeerHeading peer=Peer::Node { node } /> }.into_any(),
            None => ().into_any(),
        },
        Page::RadioSettings => view! {
            <h1>{t!(i18n, nav_radio_settings)}</h1>
        }
        .into_any(),
        Page::Preferences => view! {
            <div class="heading">
                <h1>{t!(i18n, prefs_title)}</h1>
                <span class="dim">{t!(i18n, prefs_text)}</span>
            </div>
        }
        .into_any(),
        Page::Radios => {
            let count = move || live.radios.with(|r| r.as_ref().map_or(0, Vec::len));
            view! {
                <div class="heading">
                    <h1>{t!(i18n, radios_title)}</h1>
                    <span class="dim">
                        <span class="wide-only">{move || t!(i18n, radios_count, count = count())}</span>
                        <span class="phone-only">{move || t!(i18n, radios_count_phone, count = count())}</span>
                    </span>
                </div>
                <div class="actions">
                    <button
                        class="primary with-icon"
                        title=move || t_string!(i18n, radio_add_title)
                        on:click=move |_| live.radio_editing.set(Editing::Adding)
                    >
                        <span class="icon" inner_html=PLUS></span>
                        <span class="label">{t!(i18n, radio_add_title)}</span>
                    </button>
                </div>
            }
            .into_any()
        }
        Page::Chat => match live.open.get() {
            Some(peer) => view! { <PeerHeading peer /> }.into_any(),
            None => view! { <h1>{t!(i18n, page_chat)}</h1> }.into_any(),
        },
    };
    view! {
        <header class="topbar">
            <button
                class="menu-toggle"
                aria-label=move || t_string!(i18n, nav_menu)
                on:click=move |_| live.menu.update(|open| *open = !*open)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 6h16M4 12h16M4 18h16"></path>
                </svg>
            </button>
            <crate::phone::PhoneBack />
            {move || crate::phone::is_tab(live.page.get()).then(|| view! { <crate::phone::RadioChip /> })}
            {heading}
        </header>
    }
}

/// The open conversation's heading, and what can be done with whom it is with.
#[component]
fn PeerHeading(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    match peer {
        Peer::Channel { channel } => view! {
            <div class="heading">
                <h1>{move || title(live, peer)}</h1>
                <span class="dim">{t!(i18n, nav_channel_slot, index = channel)}</span>
            </div>
        }
        .into_any(),
        Peer::Node { node } => {
            let known = move || live.nodes.with(|nodes| nodes.get(&node).cloned());
            let local = move || {
                live.status
                    .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
                    == Some(node)
            };
            // On its page, what it is: how it is doing is in the page below.
            // Elsewhere, how it is reached and when it was last heard.
            let detail = move || {
                let known = known()?;
                let mut parts: Vec<AnyView> = vec![known.id.clone().into_any()];
                if live.page.get() == Page::Node {
                    // Its role by name; its code beside, where there is room.
                    if let Some(role) = known.role.clone() {
                        let name = crate::names::role_name(i18n.get_locale(), &role);
                        parts.push(
                            view! {
                                {name}
                                <span class="code">" (" {role} ")"</span>
                            }
                            .into_any(),
                        );
                    }
                    if let Some(hardware) = &known.hw_model {
                        parts.push(model(hardware).into_any());
                    }
                } else {
                    if let Some(reach) = reach_words(i18n, &known, local()) {
                        parts.push(reach.into_any());
                    }
                    if let Some(heard) = known.last_heard {
                        parts.push(ago(live.now.get(), heard).into_any());
                    }
                }
                let mut joined = Vec::with_capacity(parts.len() * 2);
                for (index, part) in parts.into_iter().enumerate() {
                    if index > 0 {
                        joined.push(" · ".into_any());
                    }
                    joined.push(part);
                }
                Some(joined.collect_view())
            };
            let class = move || {
                let mut class = known().map_or_else(
                    || "node-avatar".to_owned(),
                    |known| avatar_class(&known, local()),
                );
                if badge::is_emoji(&short_name(live, node)) {
                    class.push_str(" emoji");
                }
                class
            };
            let favorite = move || known().is_some_and(|n| n.favorite);
            let unmessagable = move || known().is_some_and(|n| n.unmessagable);
            // A favourite of the radio's, or not: marked here, on a Meshtastic
            // radio, for any node but its own.
            let can_favor = move || {
                live.selected_kind() == Some(RadioKind::Meshtastic) && !local() && known().is_some()
            };
            let toggle_favorite = move |_| {
                let Some(radio) = live.selected.get_untracked() else {
                    return;
                };
                let favorite = !favorite();
                spawn_local(async move {
                    let _ = daemon::set_favorite(radio, node, favorite).await;
                });
            };
            let star = move || {
                if can_favor() {
                    let label = move || {
                        if favorite() {
                            t_string!(i18n, node_unfavorite)
                        } else {
                            t_string!(i18n, node_favorite)
                        }
                    };
                    view! {
                        <button
                            class="favorite"
                            class:on=favorite
                            aria-pressed=move || favorite().to_string()
                            title=label
                            aria-label=label
                            inner_html=STAR
                            on:click=toggle_favorite
                        ></button>
                    }
                    .into_any()
                } else {
                    favorite()
                        .then(|| view! { <span class="mark star" inner_html=STAR></span> })
                        .into_any()
                }
            };
            // Nobody reads what is sent to it, it tells: marked, and its chat
            // is not the first thing offered.
            let muted = move || {
                unmessagable().then(|| {
                    view! {
                        <span
                            class="mark muted"
                            title=move || t_string!(i18n, node_unmessagable)
                            aria-label=move || t_string!(i18n, node_unmessagable)
                            inner_html=MESSAGE_OFF
                        ></span>
                    }
                })
            };
            // Where else there is to go with the node: from the chat, its page;
            // from its page, the chat. On the map its card has both already.
            let action = move || {
                let page = live.page.get();
                let chat = (page == Page::Node).then(|| {
                    if unmessagable() {
                        view! {
                            <button
                                class="with-icon"
                                title=move || t_string!(i18n, node_unmessagable)
                                on:click=move |_| live.open_chat(peer)
                            >
                                <span class="icon" inner_html=MESSAGE_OFF></span>
                                <span class="label">{t!(i18n, node_message)}</span>
                            </button>
                        }
                        .into_any()
                    } else {
                        view! {
                            <button
                                class="primary with-icon"
                                title=move || t_string!(i18n, node_message)
                                on:click=move |_| live.open_chat(peer)
                            >
                                <span class="icon" inner_html=MESSAGE></span>
                                <span class="label">{t!(i18n, node_message)}</span>
                            </button>
                        }
                        .into_any()
                    }
                });
                let profile = (page == Page::Chat).then(|| {
                    view! {
                        <button on:click=move |_| live.open_node(node)>{t!(i18n, node_page)}</button>
                    }
                });
                // What it was asked for, on its page: in a drawer.
                let history = (page == Page::Node && !local()).then(|| {
                    view! {
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, requests_title)
                            aria-label=move || t_string!(i18n, requests_title)
                            on:click=move |_| live.history.set(true)
                        >
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M12 8v4l2 2"></path>
                                <path d="M3.05 11a9 9 0 1 1 .5 4m-.5 5v-5h5"></path>
                            </svg>
                        </button>
                    }
                });
                view! { {history} {profile} {chat} }
            };
            // Back to where its page was opened from, on its page.
            let back = move || {
                (live.page.get() == Page::Node).then(|| {
                    let label = move || {
                        let to = live
                            .back
                            .with(|back| back.last().copied())
                            .unwrap_or(Back::Page(Page::Nodes));
                        match to {
                            Back::Node(other) => t_string!(i18n, back_node)
                                .replace("{name}", &title(live, Peer::Node { node: other })),
                            Back::Page(Page::Map) => t_string!(i18n, back_map).to_owned(),
                            Back::Page(Page::Chat) => t_string!(i18n, back_chat).to_owned(),
                            Back::Page(Page::Packets) => t_string!(i18n, back_packets).to_owned(),
                            Back::Page(_) => t_string!(i18n, back_nodes).to_owned(),
                        }
                    };
                    view! {
                        <button
                            class="icon-button back"
                            title=label
                            aria-label=label
                            on:click=move |_| live.go_back()
                        >
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M5 12h14M5 12l6 6M5 12l6 -6"></path>
                            </svg>
                        </button>
                    }
                })
            };
            view! {
                {back}
                <span class=class>{move || badge::fit(&short_name(live, node))}</span>
                <div class="heading">
                    <h1>
                        <span class="title-text">{move || title(live, peer)}</span>
                        {star}
                        {muted}
                    </h1>
                    <span class="dim">{detail}</span>
                </div>
                <div class="actions">{action}</div>
            }
            .into_any()
        }
    }
}

/// What a daemon with no radios yet shows.
#[component]
fn Welcome() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <section class="card welcome">
            <h2>{t!(i18n, welcome_title)}</h2>
            <p>{t!(i18n, welcome_text)}</p>
            <RadioEditor id=None />
        </section>
    }
}

/// The class for a radio's link.
pub fn link_class(link: LinkState) -> &'static str {
    match link {
        LinkState::Ready => "ready",
        LinkState::Syncing => "syncing",
        LinkState::Connecting => "connecting",
        LinkState::Disconnected => "radio-down",
        LinkState::Disabled => "disabled",
    }
}

/// A radio's link, in words.
#[component]
pub fn LinkLabel(#[prop(into)] link: Signal<LinkState>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || match link.get() {
        LinkState::Disabled => t!(i18n, link_disabled).into_any(),
        _ if live.daemon.get() != Daemon::Connected => t!(i18n, link_daemon_down).into_any(),
        LinkState::Ready => t!(i18n, link_ready).into_any(),
        LinkState::Syncing => t!(i18n, link_syncing).into_any(),
        LinkState::Connecting => t!(i18n, link_connecting).into_any(),
        LinkState::Disconnected => t!(i18n, link_radio_down).into_any(),
    }
}

#[component]
fn TokenForm() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let value = RwSignal::new(String::new());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        daemon::set_token(value.get_untracked().trim());
        live.daemon.set(Daemon::Connecting);
    };
    view! {
        <form class="token card" on:submit=submit>
            <p>{t!(i18n, token_needed)}</p>
            <input
                type="password"
                autocomplete="current-password"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
            <button type="submit">{t!(i18n, token_use)}</button>
        </form>
    }
}
