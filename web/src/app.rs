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
use crate::chat::{ChatView, node_name, peer_of, short_name, title};
use crate::daemon::{self, Events, Failure};
use crate::empty::EmptyState;
use crate::i18n::{I18nContextProvider, Locale, t, t_string, use_i18n};
use crate::map::MapView;
use crate::mesh::{MESSAGE_OFF, MeshView, STAR, ago, avatar_style, model, reach_words};
use crate::node::NodeView;
use crate::packets::{KEPT, PacketsView};
use crate::preferences::PreferencesView;
use crate::radios::{Editing, RadiosView};
use crate::settings::SettingsView;
use crate::traces::{Traceroute, TracerouteStatus};
use gloo_timers::future::sleep;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::ServerMessage;
use meshctl_api::{
    Accent, Channel, Conversation, Language, LinkState, MapColors, MapEngine, MapTheme, Message,
    NewRequest, Node, NodeCheck, NodeLatest, Pace, Packet, PacketFilter, Peer, Preferences, Radio,
    RadioId, RadioKind, Request, RequestKind, RequestStatus, SightingId, Status, Theme,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

/// How long to wait before trying the daemon again.
const RETRY: Duration = Duration::from_secs(3);

/// How often "heard 5 min ago" is worked out again: often enough for what
/// is awaited to be seen going, five seconds at a time.
const CLOCK_TICK: Duration = Duration::from_secs(5);

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

/// How many pages' scrolling is kept: those steps back, and the pages.
const SCROLLS_KEPT: usize = 40;

/// What scrolls the page.
fn scroller() -> Option<web_sys::Element> {
    document().query_selector("main").ok().flatten()
}

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
    /// The open node's settings, as its admin over the mesh: the same as
    /// the radio's own.
    NodeSettings,
    /// meshctl's: how the interface looks.
    Preferences,
    /// meshctl's: the radios it connects to.
    Radios,
    /// meshctl's: what it answers on its own, and where.
    AutoReplies,
    /// meshctl's: what it notifies of.
    Notifications,
    /// meshctl's: what it keeps, and for how long.
    Data,
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
        matches!(
            self,
            Self::Preferences | Self::Radios | Self::AutoReplies | Self::Notifications | Self::Data
        )
    }
}

/// A line of a node's console: a command sent, or what it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleLine {
    pub radio: RadioId,
    pub node: u32,
    /// A command sent, rather than an answer.
    pub mine: bool,
    pub text: String,
    /// In seconds since the epoch.
    pub at: i64,
}

/// A room's login, as the interface knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomState {
    pub node: u32,
    /// As last read or made; none until read.
    pub login: Option<meshctl_api::NodeLogin>,
    /// Whether a login is under way.
    pub busy: bool,
    /// When, by this browser's clock in milliseconds, the login under way
    /// stops being waited for, and for how long in all, once the daemon says.
    pub until: Option<(f64, f64)>,
    /// Why the last login failed.
    pub error: Option<Failure>,
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
    /// The radio settings' tab, kept as it was left or as a shortcut asks.
    pub settings_tab: RwSignal<crate::settings::Tab>,
    /// The same, for a MeshCore radio, whose tabs are its own.
    pub meshcore_tab: RwSignal<crate::meshcore_settings::MeshcoreTab>,
    /// What the radios' editor is open on.
    pub radio_editing: RwSignal<Editing>,
    /// The automatic replies, in order, once read.
    pub auto_replies: RwSignal<Option<Vec<meshctl_api::AutoReply>>>,
    /// What the automatic replies' editor is open on.
    pub reply_editing: RwSignal<crate::auto_replies::ReplyEditing>,
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
    /// Whether the open conversation is being read: shown, and down at its
    /// newest. What comes to it then is read as it comes.
    pub reading: RwSignal<bool>,
    /// The emojis picked most, as the daemon counts them: none until the
    /// picker first needs them.
    pub emojis: RwSignal<Option<Vec<String>>>,
    /// The conversation open in the chat, if any.
    pub open: RwSignal<Option<Peer>>,
    /// The messages of the open conversation, oldest first.
    pub thread: RwSignal<Vec<Message>>,
    /// Of the selected radio, the newest first.
    pub traceroutes: RwSignal<Vec<Traceroute>>,
    /// The selected radio's waypoints, those not gone.
    pub waypoints: RwSignal<Vec<meshctl_api::waypoints::Waypoint>>,
    /// A waypoint being shared, where, or changed, which: its form open.
    pub waypoint_form: RwSignal<Option<crate::waypoints::Form>>,
    /// What the selected radio heard, the newest first.
    pub packets: RwSignal<Vec<Packet>>,
    /// Which packets the live view shows, but for the radio's own: those go
    /// by the preferences, as its node changes with the radio.
    pub packet_filter: RwSignal<PacketFilter>,
    /// Packets come while the live view holds still, the newest first: to
    /// go on top of it once it goes on.
    pub packets_held: RwSignal<Vec<Packet>>,
    /// Whether the live view was paused.
    pub packets_paused: RwSignal<bool>,
    /// Whether the live view is scrolled away from its top, being read.
    pub packets_away: RwSignal<bool>,
    /// Whether there is nothing older than what the live view has.
    pub packets_end: RwSignal<bool>,
    /// The newest packet when the live view was last read or opened: those
    /// after it are new, and show it as they come.
    pub packets_mark: RwSignal<SightingId>,
    /// The node whose page is open, if any.
    pub node: RwSignal<Option<u32>>,
    /// Where the open node's page was opened from, the latest last.
    pub back: RwSignal<Vec<Back>>,
    /// How far down each page left was scrolled, the latest last: to come
    /// back to it there, and to open any other at its top.
    scrolls: StoredValue<Vec<(Back, i32)>>,
    /// Which scrolling is the latest: one put off while the page is drawn
    /// gives way to another.
    scroll_turn: StoredValue<u32>,
    /// The last page out of the settings, to go back to from them on a wider
    /// screen.
    pub came_from: RwSignal<Page>,
    /// Whether the open node's removal is offered, in its drawer.
    pub removing: RwSignal<bool>,
    /// Whether the window to add from a link is open.
    pub adding_link: RwSignal<Option<crate::links::LinkWant>>,
    /// A link to add offered from elsewhere, as a contact or a channel
    /// shared in a conversation: the dialog that adds links opens with it.
    pub offered_link: RwSignal<Option<String>>,
    /// A place to show on the map, not a node's.
    pub map_spot: RwSignal<Option<(f64, f64)>>,
    /// A waypoint to show on the map, with its popup open.
    pub map_waypoint: RwSignal<Option<u32>>,
    /// What happened to waypoints, told of over the panel for a while.
    pub notices: RwSignal<Vec<crate::waypoints::Notice>>,
    /// Whether the about window is open.
    pub about: RwSignal<bool>,
    /// meshctl's version, and the commit it was built from, once asked.
    pub version: RwSignal<Option<String>>,
    /// A link's LoRa settings, taken for the LoRa tab to put in its form.
    pub lora_from_link: RwSignal<Option<meshctl_api::LoraSettings>>,
    /// Turned whenever what meshctl keeps was cleaned, or how long it keeps
    /// it changed: for the Data page to read it again.
    pub storage_turn: RwSignal<u32>,
    /// The interface the daemon served when first heard, which is this one.
    pub interface: StoredValue<Option<String>>,
    /// Whether the daemon serves another one now: this page is to be
    /// reloaded.
    pub newer: RwSignal<bool>,
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
    /// The request or traceroute button pressed last, and on whose page:
    /// the one that shows the radio's wait filling up, kept as pages come
    /// and go.
    pub pressed: RwSignal<Option<(u32, crate::node::ButtonId)>>,
    /// Why the last request or traceroute was not sent.
    pub ask_error: RwSignal<Option<Failure>>,
    /// Whether the token given, typed or kept in this browser, was refused.
    pub token_refused: RwSignal<bool>,
    /// The MeshCore nodes of the selected radio it logs in to now, as asked
    /// for something first: until when, by this browser's clock, in
    /// milliseconds, and how long in all.
    pub logging_in: RwSignal<HashMap<u32, (f64, f64)>>,
    /// The nodes the last login to, as asked for something, failed: why,
    /// kept until one goes in.
    pub login_failed: RwSignal<HashMap<u32, Failure>>,
    /// When, by this browser's clock, the selected radio sends each paced
    /// kind of thing again: until then it would drop it.
    pub ready: RwSignal<HashMap<Pace, u32>>,
    /// Bumped each time the selected radio syncs, for what is read only then.
    pub synced: RwSignal<u32>,
    /// Seconds since the epoch, ticking.
    pub now: RwSignal<u32>,
    /// The open room's login: as read or made, while one is under way, or
    /// why the last failed; none while no room is open.
    pub room: RwSignal<Option<RoomState>>,
    /// What was said with the consoles of nodes the radios are logged in to
    /// as their admin, since the page opened: each radio's, each node's.
    pub console: RwSignal<Vec<ConsoleLine>>,
}

impl Live {
    fn new() -> Self {
        Self {
            daemon: RwSignal::new(Daemon::Connecting),
            page: RwSignal::new(Page::Map),
            menu: RwSignal::new(false),
            preferences: RwSignal::new(shown()),
            radios: RwSignal::new(None),
            selected: RwSignal::new(daemon::last_radio()),
            settings_tab: RwSignal::new(crate::settings::Tab::User),
            meshcore_tab: RwSignal::new(crate::meshcore_settings::MeshcoreTab::User),
            radio_editing: RwSignal::new(Editing::Closed),
            auto_replies: RwSignal::new(None),
            reply_editing: RwSignal::new(crate::auto_replies::ReplyEditing::Closed),
            status: RwSignal::new(None),
            nodes: RwSignal::new(BTreeMap::new()),
            loaded: RwSignal::new(None),
            channels: RwSignal::new(Vec::new()),
            conversations: RwSignal::new(Vec::new()),
            reading: RwSignal::new(false),
            emojis: RwSignal::new(None),
            open: RwSignal::new(None),
            thread: RwSignal::new(Vec::new()),
            traceroutes: RwSignal::new(Vec::new()),
            waypoints: RwSignal::new(Vec::new()),
            waypoint_form: RwSignal::new(None),
            map_waypoint: RwSignal::new(None),
            notices: RwSignal::new(Vec::new()),
            packets: RwSignal::new(Vec::new()),
            packet_filter: RwSignal::new(PacketFilter::default()),
            packets_held: RwSignal::new(Vec::new()),
            packets_paused: RwSignal::new(false),
            packets_away: RwSignal::new(false),
            packets_end: RwSignal::new(false),
            packets_mark: RwSignal::new(0),
            node: RwSignal::new(None),
            back: RwSignal::new(Vec::new()),
            scrolls: StoredValue::new(Vec::new()),
            scroll_turn: StoredValue::new(0),
            came_from: RwSignal::new(Page::Map),
            history: RwSignal::new(false),
            removing: RwSignal::new(false),
            adding_link: RwSignal::new(None),
            offered_link: RwSignal::new(None),
            map_spot: RwSignal::new(None),
            about: RwSignal::new(false),
            version: RwSignal::new(None),
            lora_from_link: RwSignal::new(None),
            storage_turn: RwSignal::new(0),
            interface: StoredValue::new(None),
            newer: RwSignal::new(false),
            radio_sheet: RwSignal::new(false),
            map_pick: RwSignal::new(None),
            map_focus: RwSignal::new(None),
            map_route: RwSignal::new(None),
            requests: RwSignal::new(Vec::new()),
            latest: RwSignal::new(None),
            checks: RwSignal::new(Vec::new()),
            pressed: RwSignal::new(None),
            ask_error: RwSignal::new(None),
            token_refused: RwSignal::new(false),
            logging_in: RwSignal::new(HashMap::new()),
            login_failed: RwSignal::new(HashMap::new()),
            ready: RwSignal::new(HashMap::new()),
            synced: RwSignal::new(0),
            now: RwSignal::new(now()),
            room: RwSignal::new(None),
            console: RwSignal::new(Vec::new()),
        }
    }

    /// Whether the live view leaves out what the radio itself sends, as the
    /// preferences say: so by default.
    pub fn packets_hide_own(self) -> bool {
        !self.preferences.with_untracked(|p| p.packets_with_own)
    }

    /// Which packets the live view shows, the radio's own left out if asked
    /// and its node is known.
    pub fn packets_filter(self) -> PacketFilter {
        let own = self
            .status
            .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        PacketFilter {
            not_from: own.filter(|_| self.packets_hide_own()),
            ..self.packet_filter.get_untracked()
        }
    }

    /// Whether what comes waits apart: the live view paused, or being read
    /// away from its top.
    pub fn packets_holding(self) -> bool {
        self.packets_paused.get_untracked() || self.packets_away.get_untracked()
    }

    /// What came while the live view held still, on top of it.
    pub fn release_packets(self) {
        let held = self
            .packets_held
            .try_update(std::mem::take)
            .unwrap_or_default();
        if held.is_empty() {
            return;
        }
        self.packets.update(|packets| {
            packets.splice(0..0, held);
            packets.truncate(KEPT);
        });
    }

    /// The live view read again, as its filters say: what was held goes, as
    /// it may not be let through now.
    pub async fn reload_packets(self) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            return Ok(());
        };
        let filter = self.packets_filter();
        let packets = daemon::packets(radio, None, &filter).await?;
        if self.selected.get_untracked() == Some(radio) && self.packets_filter() == filter {
            self.set_packets(packets);
        }
        Ok(())
    }

    /// The live view's packets, read anew: none of them new.
    fn set_packets(self, packets: Vec<Packet>) {
        self.packets_end.set(packets.len() < daemon::PACKETS_PAGE);
        self.packets_mark.set(
            packets
                .first()
                .map_or(self.packets_mark.get_untracked(), |p| p.id),
        );
        self.packets_held.set(Vec::new());
        self.packets.set(packets);
    }

    /// Shows a page at its top, and closes the bars over it on a narrow
    /// screen.
    pub fn go(self, page: Page) {
        self.leave();
        self.open_page(page);
        self.scroll_to(0);
    }

    /// Goes back to a page: where it was left, scrolled as it was.
    pub fn go_back_to(self, page: Page) {
        self.leave();
        let top = self.left_at(Back::Page(page));
        self.open_page(page);
        self.scroll_to(top);
    }

    /// The page shown, as a step to go back to.
    fn here(self) -> Back {
        match (self.page.get_untracked(), self.node.get_untracked()) {
            (Page::Node, Some(node)) => Back::Node(node),
            (page, _) => Back::Page(page),
        }
    }

    /// Keeps how far down the page shown is scrolled, to come back to it
    /// there.
    fn leave(self) {
        let Some(main) = scroller() else { return };
        let here = self.here();
        let top = main.scroll_top();
        self.scrolls.update_value(|scrolls| {
            scrolls.retain(|(step, _)| *step != here);
            scrolls.push((here, top));
            if scrolls.len() > SCROLLS_KEPT {
                scrolls.remove(0);
            }
        });
    }

    /// How far down a page was left; at its top, if it never was.
    fn left_at(self, step: Back) -> i32 {
        self.scrolls.with_value(|scrolls| {
            scrolls
                .iter()
                .find(|(left, _)| *left == step)
                .map_or(0, |(_, top)| *top)
        })
    }

    /// Scrolls the page to `top` once it is drawn: tried again for a moment
    /// while what it shows is read and it is not that long yet.
    fn scroll_to(self, top: i32) {
        let turn = self.scroll_turn.get_value().wrapping_add(1);
        self.scroll_turn.set_value(turn);
        fn attempt(live: Live, turn: u32, top: i32, left: u32) {
            if live.scroll_turn.get_value() != turn {
                return;
            }
            let Some(main) = scroller() else { return };
            main.set_scroll_top(top);
            if main.scroll_top() + 1 < top && left > 0 {
                set_timeout(
                    move || attempt(live, turn, top, left - 1),
                    Duration::from_millis(50),
                );
            }
        }
        request_animation_frame(move || attempt(self, turn, top, 20));
    }

    /// Shows a page, as it is, and closes the bars over it on a narrow
    /// screen.
    fn open_page(self, page: Page) {
        let from = self.page.get_untracked();
        if crate::phone::back_of(from) != Some(Page::More) && from != Page::More {
            self.came_from.set(from);
        }
        self.page.set(page);
        self.menu.set(false);
        // A page gone to anew: nothing to go back to from it.
        self.back.set(Vec::new());
    }

    /// Shows a place on the map, as one shared in a conversation: no node
    /// picked, and the map gone to it with a pin.
    pub fn show_place(self, lat: f64, lon: f64) {
        self.map_pick.set(None);
        self.map_spot.set(Some((lat, lon)));
        self.go(Page::Map);
    }

    /// Shows a waypoint on the map, with its popup open, the map gone to it.
    pub fn show_waypoint(self, id: u32) {
        self.map_pick.set(None);
        self.map_waypoint.set(Some(id));
        self.go(Page::Map);
    }

    /// The language the interface speaks: the one kept, or the browser's.
    pub fn locale(self) -> Locale {
        match self.preferences.with_untracked(|p| p.language) {
            Some(Language::En) => Locale::en,
            Some(Language::Es) => Locale::es,
            None => browser_locale(),
        }
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
    /// Where it was left, scrolled as it was.
    pub fn go_back(self) {
        self.leave();
        let mut back = self.back.get_untracked();
        let to = back.pop().unwrap_or(Back::Page(Page::Nodes));
        let top = self.left_at(to);
        match to {
            Back::Node(node) => {
                self.show_node(node);
                self.back.set(back);
            }
            Back::Page(page) => self.open_page(page),
        }
        self.scroll_to(top);
    }

    pub fn select(self, radio: RadioId) {
        daemon::set_last_radio(radio);
        self.back.set(Vec::new());
        // What there is of the last one goes: nothing of it is shown as the
        // new one's while that is read.
        self.loaded.set(None);
        self.status.set(None);
        self.logging_in.set(HashMap::new());
        self.login_failed.set(HashMap::new());
        self.nodes.set(BTreeMap::new());
        self.selected.set(Some(radio));
        self.open.set(None);
        self.thread.set(Vec::new());
        self.node.set(None);
        self.requests.set(Vec::new());
        self.map_pick.set(None);
        self.map_route.set(None);
        // The live view's channel and node were the last one's.
        self.packet_filter.update(|f| {
            f.channel = None;
            f.node = None;
        });
        self.packets_held.set(Vec::new());
        if matches!(self.page.get_untracked(), Page::Chat | Page::Node)
            || self.page.get_untracked().is_meshctl()
        {
            self.page.set(Page::Map);
        }
        self.menu.set(false);
        spawn_local(async move {
            let _ = self.refresh_selected().await;
            // The one left, its unread as they are now.
            let _ = self.refresh_radios().await;
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

    /// How many messages a radio has unread: the selected one's as its
    /// conversations say now, any other's as the daemon said last.
    pub fn unread(self, id: RadioId) -> u32 {
        if self.selected.get() == Some(id) {
            self.conversations
                .with(|c| c.iter().filter(|c| !c.muted).map(|c| c.unread).sum())
        } else {
            self.radios.with(|radios| {
                radios
                    .iter()
                    .flatten()
                    .find(|r| r.id == id)
                    .map_or(0, |r| r.unread)
            })
        }
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
            self.waypoints.set(Vec::new());
            return Ok(());
        };
        let status = daemon::status(radio).await?;
        let nodes = daemon::nodes(radio).await?;
        let channels = daemon::channels(radio).await?;
        let conversations = daemon::conversations(radio).await?;
        let traceroutes = daemon::traceroutes(radio).await?;
        let waypoints = daemon::waypoints(radio).await?;
        // The live view's, as its filters say: the radio's own node, from
        // what was just read of it.
        let filter = PacketFilter {
            not_from: status
                .local
                .as_ref()
                .map(|l| l.num)
                .filter(|_| self.packets_hide_own()),
            ..self.packet_filter.get_untracked()
        };
        let packets = daemon::packets(radio, None, &filter).await?;
        // The selection may have moved on while we were asking.
        if self.selected.get_untracked() == Some(radio) {
            self.status.set(Some(status));
            self.nodes
                .set(nodes.into_iter().map(|node| (node.num, node)).collect());
            self.channels.set(channels);
            self.conversations.set(conversations);
            self.traceroutes.set(traceroutes);
            self.waypoints.set(waypoints);
            self.set_packets(packets);
            self.loaded.set(Some(radio));
        }
        self.refresh_thread().await
    }

    /// Opens a node's page.
    pub fn open_node(self, node: u32) {
        self.leave();
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
        // Another node's page, at its top.
        self.scroll_to(0);
    }

    /// Shows a node's page, and reads what it shows.
    fn show_node(self, node: u32) {
        self.node.set(Some(node));
        self.history.set(false);
        self.removing.set(false);
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
                // The routes traced are kept apart, with every node's.
                let asked = requests
                    .into_iter()
                    .filter(|r| r.kind != RequestKind::Traceroute);
                self.requests.set(asked.collect());
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
        self.ask_over(node, kind, channel, None);
    }

    /// Asks a MeshCore sensor what it kept over the last `seconds`.
    pub fn ask_series(self, node: u32, seconds: u32) {
        self.ask_over(node, RequestKind::Series, 0, Some(seconds));
    }

    fn ask_over(self, node: u32, kind: RequestKind, channel: u32, seconds: Option<u32>) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        self.ask_error.set(None);
        spawn_local(async move {
            let new = NewRequest {
                node,
                kind,
                channel,
                seconds,
            };
            let done = daemon::ask(radio, &new).await;
            // A login it went first with is over, either way.
            self.logging_in.update(|l| {
                l.remove(&node);
            });
            match done {
                Ok(request) => {
                    self.login_failed.update(|l| {
                        l.remove(&node);
                    });
                    if let Some(pace) = kind.pace() {
                        self.paced(pace);
                    }
                    self.keep_request(request);
                }
                Err(failure) => {
                    if login_failure(&failure) {
                        self.login_failed.update(|l| {
                            l.insert(node, failure);
                        });
                    }
                    self.refused(radio, failure);
                }
            }
        });
    }

    /// Whether the radio logs in to a node now, as asked for something.
    pub fn logging_in_to(self, node: u32) -> bool {
        self.logging_in.with(|l| l.contains_key(&node))
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
                    // A MeshCore radio sends one whenever asked.
                    if self.selected_kind() != Some(meshctl_api::RadioKind::Meshcore) {
                        self.paced(Pace::Traceroute);
                    }
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
    /// Whether a node of the selected radio is a MeshCore room.
    pub fn is_room(self, node: u32) -> bool {
        self.nodes
            .with(|nodes| nodes.get(&node).and_then(|n| n.role.as_deref()) == Some("ROOM"))
    }

    /// Whether a node of the selected radio is one to write to. A MeshCore
    /// repeater or sensor is not: it drops what it is sent, and takes it as a
    /// command only from its admin, which its console is for.
    pub fn talks(self, node: u32) -> bool {
        self.selected_kind() != Some(RadioKind::Meshcore)
            || !self.nodes.with(|nodes| {
                matches!(
                    nodes.get(&node).and_then(|n| n.role.as_deref()),
                    Some("REPEATER" | "SENSOR")
                )
            })
    }

    /// Reads a room's login, as it is opened or the radio syncs again; not
    /// logged in since the radio connected, with a password kept, it logs in
    /// with it, as once each time the radio connects.
    pub fn read_room(self, node: u32) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        let same = self
            .room
            .with_untracked(|r| r.as_ref().is_some_and(|r| r.node == node));
        if !same {
            self.room.set(Some(RoomState {
                node,
                login: None,
                busy: false,
                until: None,
                error: None,
            }));
        }
        spawn_local(async move {
            let Ok(login) = daemon::node_login(radio, node).await else {
                return;
            };
            let again = login.access.is_none() && login.remembered;
            self.room.update(|r| {
                if let Some(r) = r.as_mut().filter(|r| r.node == node) {
                    r.login = Some(login);
                }
            });
            if again {
                self.log_in_node(node, None);
            }
        });
    }

    /// Logs the selected radio in to a room: with a password, or with the
    /// one kept, or as a guest.
    pub fn log_in_node(self, node: u32, password: Option<String>) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        let mark =
            move |busy: bool, error: Option<Failure>, login: Option<meshctl_api::NodeLogin>| {
                self.room.update(|r| {
                    if let Some(r) = r.as_mut().filter(|r| r.node == node) {
                        r.busy = busy;
                        r.error = error;
                        if login.is_some() {
                            r.login = login;
                        }
                    }
                });
            };
        mark(true, None, None);
        spawn_local(async move {
            match daemon::log_in_node(radio, node, password).await {
                Ok(login) => mark(false, None, Some(login)),
                Err(failure) => mark(false, Some(failure), None),
            }
        });
    }

    /// Forgets the password kept for a room, once sure; the login made with
    /// it stays until the radio connects again.
    pub fn forget_node_password(self, node: u32, sure: &str) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        let sure = web_sys::window()
            .and_then(|w| w.confirm_with_message(sure).ok())
            .unwrap_or(false);
        if !sure {
            return;
        }
        spawn_local(async move {
            if let Ok(login) = daemon::forget_node_password(radio, node).await {
                self.room.update(|r| {
                    if let Some(r) = r.as_mut().filter(|r| r.node == node) {
                        r.login = Some(login);
                    }
                });
            }
        });
    }

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
            Ok((events, teller)) => {
                crate::notify::connected(self, teller);
                events
            }
            Err(failure) => return failure,
        };
        // After ready, not before: nothing is lost in between.
        if let Err(failure) = self.refresh().await {
            return failure;
        }
        self.daemon.set(Daemon::Connected);
        crate::notify::ready(self);

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
                                // By this browser's clock until it is read
                                // again, as it is once in sync.
                                status.since = Some(i64::from(now()));
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
                // Forgotten by the radio: out of the list, and out of its page
                // if it was open, back where it was opened from.
                ServerMessage::NodeRemoved { radio, node } if self.is_selected(radio) => {
                    self.nodes.update(|nodes| {
                        nodes.remove(&node);
                    });
                    if self.page.get_untracked() == Page::Node
                        && self.node.get_untracked() == Some(node)
                    {
                        self.go_back();
                    }
                    Ok(())
                }
                // All kept of it forgotten: out of what this page has too.
                ServerMessage::ConversationRead { radio, .. }
                | ServerMessage::ConversationMuted { radio, .. }
                    if self.is_selected(radio) =>
                {
                    self.refresh_conversations().await
                }
                // Cleaned away: what shows what was kept, read again.
                ServerMessage::Cleaned { .. } => {
                    self.storage_turn.update(|n| *n += 1);
                    self.refresh_selected().await
                }
                ServerMessage::Retention { .. } => {
                    self.storage_turn.update(|n| *n += 1);
                    Ok(())
                }
                ServerMessage::MessagesForgotten { radio, ids } if self.is_selected(radio) => {
                    self.thread.update(|t| t.retain(|m| !ids.contains(&m.id)));
                    Ok(())
                }
                // A channel emptied, or a conversation with a node gone.
                ServerMessage::ConversationForgotten { radio, peer } if self.is_selected(radio) => {
                    // A channel or a room is emptied and stays; a
                    // conversation with a node goes.
                    let goes = matches!(peer, Peer::Node { node } if !self.is_room(node));
                    if self.open.get_untracked() == Some(peer) {
                        self.thread.set(Vec::new());
                        if goes {
                            self.open.set(None);
                            if self.page.get_untracked() == Page::Chat {
                                self.go(Page::Chats);
                            }
                        }
                    }
                    if goes {
                        self.conversations.update(|c| c.retain(|c| c.peer != peer));
                    }
                    self.refresh_conversations().await
                }
                ServerMessage::NodeForgotten { radio, node } if self.is_selected(radio) => {
                    let peer = Peer::Node { node };
                    self.requests.update(|r| r.retain(|r| r.node != node));
                    self.traceroutes.update(|t| t.retain(|t| t.target != node));
                    self.conversations.update(|c| c.retain(|c| c.peer != peer));
                    self.packets.update(|p| p.retain(|p| p.from != node));
                    self.packets_held.update(|p| p.retain(|p| p.from != node));
                    if self.open.get_untracked() == Some(peer) {
                        self.open.set(None);
                        self.thread.set(Vec::new());
                        if self.page.get_untracked() == Page::Chat {
                            self.go(Page::Chats);
                        }
                    }
                    Ok(())
                }
                ServerMessage::Waypoint { radio, waypoint } if self.is_selected(radio) => {
                    self.waypoints.update(|waypoints| {
                        waypoints.retain(|w| w.id != waypoint.id);
                        waypoints.insert(0, waypoint);
                    });
                    Ok(())
                }
                ServerMessage::WaypointGone { radio, id } if self.is_selected(radio) => {
                    self.waypoints
                        .update(|waypoints| waypoints.retain(|w| w.id != id));
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
                ServerMessage::MessageHeard {
                    radio,
                    id,
                    heard,
                    repeaters,
                    scope,
                } if self.is_selected(radio) => {
                    self.thread.update(|thread| {
                        if let Some(message) = thread.iter_mut().find(|m| m.id == id) {
                            message.heard = heard;
                            message.repeaters = repeaters;
                            message.scope = scope;
                        }
                    });
                    Ok(())
                }
                ServerMessage::Console {
                    radio,
                    node,
                    text,
                    at,
                } => {
                    self.console.update(|lines| {
                        lines.push(ConsoleLine {
                            radio,
                            node,
                            mine: false,
                            text,
                            at,
                        });
                    });
                    Ok(())
                }
                ServerMessage::LoginWaiting {
                    radio,
                    node,
                    wait_ms,
                } if self.is_selected(radio) => {
                    let until = js_sys::Date::now() + f64::from(wait_ms);
                    // Any login, as one asked for something goes first.
                    self.logging_in.update(|l| {
                        l.insert(node, (until, f64::from(wait_ms)));
                    });
                    self.room.update(|r| {
                        if let Some(r) = r.as_mut().filter(|r| r.node == node && r.busy) {
                            r.until = Some((until, f64::from(wait_ms)));
                        }
                    });
                    Ok(())
                }
                ServerMessage::Request { radio, request } if self.is_selected(radio) => {
                    if request.status == RequestStatus::Pending
                        && let Some(pace) = request.kind.pace()
                    {
                        self.paced(pace);
                    }
                    // A traceroute among the routes traced, of any node, for
                    // the map too; the rest, the open node's.
                    match Traceroute::of(&request) {
                        Some(traceroute) => self.keep_traceroute(traceroute),
                        None => self.keep_request(request),
                    }
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
                    // In the live view if it lets it through: on top, or apart
                    // while it holds still. One already there, as MeshCore's
                    // is when another copy of it comes, stays where it is.
                    if self.packets_filter().lets(&packet) {
                        let id = packet.id;
                        let at = |list: RwSignal<Vec<Packet>>| {
                            list.with_untracked(|l| l.iter().position(|p| p.id == id))
                                .map(|at| (list, at))
                        };
                        if let Some((list, at)) = at(self.packets_held).or_else(|| at(self.packets))
                        {
                            list.update(|packets| packets[at] = packet);
                        } else {
                            let list = if self.packets_holding() {
                                self.packets_held
                            } else {
                                self.packets
                            };
                            list.update(|packets| {
                                packets.insert(0, packet);
                                packets.truncate(KEPT);
                            });
                        }
                    }
                    Ok(())
                }
                // The first heard is this page's; another, a newer meshctl.
                ServerMessage::Interface { build } => {
                    match self.interface.get_value() {
                        None => self.interface.set_value(Some(build)),
                        Some(first) if first != build => self.newer.set(true),
                        Some(_) => {}
                    }
                    Ok(())
                }
                ServerMessage::Preferences { preferences } => {
                    self.preferences.set(preferences);
                    Ok(())
                }
                ServerMessage::Notification { notification } => {
                    // A waypoint shared: told over the panel too. A fence
                    // crossed is, as it comes on its own.
                    if let Some(waypoint) = notification
                        .waypoint
                        .as_ref()
                        .filter(|w| w.event == meshctl_api::WaypointEvent::Shared)
                    {
                        let (icon, name) = notification
                            .text
                            .as_deref()
                            .and_then(|t| t.split_once(' '))
                            .map_or((String::new(), String::new()), |(i, n)| {
                                (i.to_owned(), n.to_owned())
                            });
                        crate::waypoints::tell(
                            self,
                            notification.radio,
                            waypoint.id,
                            notification.sender.clone(),
                            icon,
                            name,
                            waypoint.event,
                        );
                    }
                    crate::notify::show(self, notification);
                    Ok(())
                }
                ServerMessage::FenceCrossed {
                    radio,
                    waypoint,
                    crossing,
                    asked,
                    who,
                    icon,
                    name,
                } => {
                    if self.is_selected(radio) {
                        self.waypoints.update(|waypoints| {
                            if let Some(w) = waypoints.iter_mut().find(|w| w.id == waypoint) {
                                w.crossings.insert(0, crossing);
                                w.crossings.truncate(meshctl_api::waypoints::CROSSINGS_TOLD);
                            }
                        });
                    }
                    // Kept all the same; told of as the fence asks.
                    if asked {
                        crate::waypoints::tell(
                            self,
                            radio,
                            waypoint,
                            who,
                            icon,
                            name,
                            if crossing.entered {
                                meshctl_api::WaypointEvent::Entered
                            } else {
                                meshctl_api::WaypointEvent::Left
                            },
                        );
                    }
                    Ok(())
                }
                ServerMessage::Radios | ServerMessage::Lagged => self.refresh().await,
                // Another radio's messages, come or read: how many it has
                // unread, on its button.
                ServerMessage::Message { .. }
                | ServerMessage::ConversationRead { .. }
                | ServerMessage::ConversationMuted { .. } => self.refresh_radios().await,
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
                    // Refused, when one was given: not the one it asks.
                    live.token_refused.set(daemon::token().is_some());
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

/// The preferences the page came in, as the daemon put them on `<html>`: what
/// the interface starts from until it has them, so that it does not paint
/// itself in the defaults' colours in between. The rest, the defaults.
fn shown() -> Preferences {
    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element());
    let get = |name: &str| root.as_ref().and_then(|root| root.get_attribute(name));
    Preferences {
        theme: match get("data-theme").as_deref() {
            Some("light") => Theme::Light,
            Some("dark") => Theme::Dark,
            _ => Theme::System,
        },
        accent: match get("data-accent").as_deref() {
            Some("blue") => Accent::Blue,
            Some("teal") => Accent::Teal,
            Some("violet") => Accent::Violet,
            Some("coral") => Accent::Coral,
            Some("gray") => Accent::Gray,
            _ => Accent::Amber,
        },
        map_theme: match get("data-map-theme").as_deref() {
            Some("light") => MapTheme::Light,
            Some("dark") => MapTheme::Dark,
            _ => MapTheme::Page,
        },
        map_engine: match get("data-map-engine").as_deref() {
            Some("vector") => MapEngine::Vector,
            _ => MapEngine::Png,
        },
        map_colors: match get("data-map-colors").as_deref() {
            Some("gray") => MapColors::Gray,
            Some("vivid") => MapColors::Vivid,
            _ => MapColors::Full,
        },
        ..Preferences::default()
    }
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
            Accent::Blue => "blue",
            Accent::Teal => "teal",
            Accent::Violet => "violet",
            Accent::Coral => "coral",
            Accent::Gray => "gray",
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
    set(
        "data-map-engine",
        match preferences.map_engine {
            MapEngine::Png => None,
            MapEngine::Vector => Some("vector"),
        },
    );
    set(
        "data-map-colors",
        match preferences.map_colors {
            MapColors::Full => None,
            MapColors::Gray => Some("gray"),
            MapColors::Vivid => Some("vivid"),
        },
    );
    // The tab's icon in the accent, drawn here: the daemon's `/favicon.svg`
    // is too, but the service worker may hand out the one it kept before.
    if let Ok(Some(icon)) = root.query_selector(r#"link[rel="icon"]"#) {
        let svg = meshctl_api::mark::accent_icon(preferences.accent);
        let _ = icon.set_attribute("href", &meshctl_api::mark::data_url(&svg));
    }
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
    crate::notify::watch(live);
    crate::phone::wire_back(live);
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

    // With no radios yet, the welcome, over the whole screen: the bars would
    // have nothing to hold. It stays while the radio added in it links, and
    // gives way to meshctl's own settings, which may be gone to before.
    let welcoming = RwSignal::new(None::<RadioId>);
    let welcome = Memo::new(move |_| {
        welcoming.get().is_some()
            || (live.radios.with(|r| r.as_ref().map(Vec::len)) == Some(0)
                && live.daemon.get() != Daemon::NeedsToken
                && !live.page.get().is_meshctl())
    });
    // Asked for its token, it too over the whole screen, as nothing else
    // can be read without it.
    let needs_token = Memo::new(move |_| live.daemon.get() == Daemon::NeedsToken);
    // Not known yet whether there are radios: neither the interface nor the
    // welcome, the one giving way to the other as soon as it is; unless the
    // daemon cannot be reached, which the interface says.
    let undecided = Memo::new(move |_| {
        live.radios.with(Option::is_none)
            && live.daemon.get() != Daemon::Unreachable
            && !live.page.get().is_meshctl()
    });
    move || {
        if needs_token.get() {
            view! { <crate::welcome::TokenScreen /> }.into_any()
        } else if undecided.get() {
            view! { <div class="welcome-screen"></div> }.into_any()
        } else if welcome.get() {
            view! { <crate::welcome::Welcome welcoming /> }.into_any()
        } else {
            view! { <Frame /> }.into_any()
        }
    }
}

/// The interface: the two bars aside, or the tabs at the foot, and the page.
#[component]
fn Frame() -> impl IntoView {
    let live = expect_context::<Live>();
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
                <crate::waypoints::Notices />
            </div>
            <crate::phone::TabBar />
            <crate::phone::RadioSheet />
            <crate::links::LinkAdder />
            // At the root, as every dialog, over all the rest: within the
            // map, the phone's tab bar went over it.
            <crate::waypoints::WaypointForm />
            <crate::about::AboutDialog />
            <Newer />
        </div>
    }
}

/// That the daemon serves a newer interface than this page runs: to reload
/// it, which it does not on its own, not to lose what is being written.
#[component]
fn Newer() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let reload = |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    };
    move || {
        live.newer.get().then(|| {
            view! {
                <div class="newer" role="status">
                    <span class="newer-words">
                        <strong>{t!(i18n, newer_title)}</strong>
                        <span>{t!(i18n, newer_text)}</span>
                    </span>
                    <button type="button" class="primary" on:click=reload>
                        {t!(i18n, newer_reload)}
                    </button>
                </div>
            }
        })
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
            (Daemon::NeedsToken, _, _) => Shown::Nothing,
            (Daemon::Unreachable, None, _) => Shown::Down,
            (_, None, _) => Shown::Nothing,
            (_, _, Page::Preferences) => Shown::Page(Page::Preferences),
            (_, _, Page::Radios) => Shown::Page(Page::Radios),
            (_, _, Page::AutoReplies) => Shown::Page(Page::AutoReplies),
            (_, _, Page::Notifications) => Shown::Page(Page::Notifications),
            (_, _, Page::Data) => Shown::Page(Page::Data),
            // The welcome, over the whole screen, instead.
            (_, Some(0), _) => Shown::Nothing,
            (_, Some(_), page) => Shown::Page(page),
        }
    });
    move || match shown.get() {
        Shown::Down => view! {
            <EmptyState
                icon=crate::empty::SERVER_OFF
                title=t!(i18n, empty_daemon_title).into_any()
                text=t!(i18n, empty_daemon_text).into_any()
            />
        }
        .into_any(),
        Shown::Nothing => ().into_any(),
        Shown::Page(Page::Preferences) => view! { <PreferencesView /> }.into_any(),
        Shown::Page(Page::Radios) => view! { <RadiosView /> }.into_any(),
        Shown::Page(Page::AutoReplies) => {
            view! { <crate::auto_replies::AutoRepliesView /> }.into_any()
        }
        Shown::Page(Page::Notifications) => {
            view! { <crate::notifications::NotificationsView /> }.into_any()
        }
        Shown::Page(Page::Data) => view! { <crate::storage::DataView /> }.into_any(),
        Shown::Page(Page::Map) => view! { <MapView /> }.into_any(),
        Shown::Page(Page::Nodes) => view! { <MeshView /> }.into_any(),
        Shown::Page(Page::Chat) => view! { <ChatView /> }.into_any(),
        Shown::Page(Page::Node) => view! { <NodeView /> }.into_any(),
        Shown::Page(Page::Packets) => view! { <PacketsView /> }.into_any(),
        Shown::Page(Page::RadioSettings) => view! { <SettingsView /> }.into_any(),
        Shown::Page(Page::NodeSettings) => {
            let live = expect_context::<Live>();
            (move || live.node.get().map(|node| view! { <SettingsView node /> })).into_any()
        }
        Shown::Page(Page::Chats) => view! { <crate::phone::ChatsView /> }.into_any(),
        Shown::Page(Page::More) => view! { <crate::phone::MoreView /> }.into_any(),
    }
}

/// What [`Content`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Down,
    Nothing,
    Page(Page),
}

const CHEVRON: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const GEAR: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/settings.svg");
const TRASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/trash.svg");
const HISTORY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/history.svg");
const BELL_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/bell-off.svg");
const DOTS_VERTICAL: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/dots-vertical.svg");
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
            <crate::about::Logo class="logo" />
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
            {move || {
                let unread = live.unread(id);
                (unread > 0).then(|| view! { <span class="unread">{unread_count(unread)}</span> })
            }}
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
            Page::Data => t!(i18n, data_title).into_any(),
            Page::AutoReplies => t!(i18n, replies_title).into_any(),
            Page::Notifications => t!(i18n, notifications_title).into_any(),
            _ => t!(i18n, radios_title).into_any(),
        };
        // The radios' page opens on their list, the editor closed; the
        // replies' too.
        let open = move |_| {
            if page == Page::Radios && live.page.get_untracked() != page {
                live.radio_editing.set(Editing::Closed);
            }
            if page == Page::AutoReplies && live.page.get_untracked() != page {
                live.reply_editing
                    .set(crate::auto_replies::ReplyEditing::Closed);
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
        <div class="sidebar-body">
            {item(Page::Radios)} {item(Page::AutoReplies)} {item(Page::Notifications)}
            {item(Page::Data)} {item(Page::Preferences)}
        </div>
        <crate::about::AboutFoot />
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
                Some(Signal::derive(move || crate::map::shown(live))),
            ),
            Page::Nodes => (
                t!(i18n, nodes_title).into_any(),
                // As many as the list has: those heard, not the radio's own.
                Some(Signal::derive(move || {
                    let local = live
                        .status
                        .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
                    live.nodes
                        .with(|nodes| nodes.keys().filter(|&&num| Some(num) != local).count())
                })),
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

    view! {
        <Show when=move || live.selected.get().is_some()>
            <RadioHead />
            // The radio's pages, always there; its conversations under
            // them, scrolled on their own however many there are.
            <nav class="sidebar-pages">
                {page_item(Page::Map)}
                {page_item(Page::Nodes)}
                {page_item(Page::Packets)}
            </nav>
            <div class="sidebar-body">
                <Conversations />
            </div>
            <LocalFoot />
        </Show>
    }
}

/// The selected radio's conversations, in their sections: its channels, its
/// rooms, if it knows any, and its direct conversations. The same in the
/// sidebar and on a phone.
#[component]
pub(crate) fn Conversations() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let channels = Signal::derive(move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| Peer::Channel { channel: c.index })
                .collect::<Vec<_>>()
        })
    });
    let directs = Signal::derive(move || {
        live.conversations.with(|conversations| {
            conversations
                .iter()
                .map(|c| c.peer)
                .filter(|peer| matches!(peer, Peer::Node { node } if !live.is_room(*node)))
                .collect::<Vec<_>>()
        })
    });
    // Every room the radio knows: those with something said in them first,
    // the latest first, then the rest by name.
    let rooms = Signal::derive(move || {
        let mut rooms: Vec<Peer> = live.conversations.with(|conversations| {
            conversations
                .iter()
                .map(|c| c.peer)
                .filter(|peer| matches!(peer, Peer::Node { node } if live.is_room(*node)))
                .collect()
        });
        let mut quiet: Vec<(String, u32)> = live.nodes.with(|nodes| {
            nodes
                .values()
                // One the radio no longer has, only with something said in it.
                .filter(|n| n.role.as_deref() == Some("ROOM") && n.on_radio != Some(false))
                .filter(|n| !rooms.contains(&Peer::Node { node: n.num }))
                .map(|n| {
                    (
                        n.long_name.clone().unwrap_or_default().to_lowercase(),
                        n.num,
                    )
                })
                .collect()
        });
        quiet.sort();
        rooms.extend(quiet.into_iter().map(|(_, node)| Peer::Node { node }));
        rooms
    });
    view! {
        <Section
            bit=SECTION_CHANNELS
            title=Signal::derive(move || t_string!(i18n, nav_channels).to_owned())
            peers=channels
        />
        <Show when=move || !rooms.with(Vec::is_empty)>
            <Section
                bit=SECTION_ROOMS
                title=Signal::derive(move || t_string!(i18n, nav_rooms).to_owned())
                peers=rooms
            />
        </Show>
        <Section
            bit=SECTION_DIRECTS
            title=Signal::derive(move || t_string!(i18n, nav_directs).to_owned())
            peers=directs
        />
        <Show when=move || {
            directs.with(Vec::is_empty) && !live.preferences.with(|p| p.sections_folded & SECTION_DIRECTS != 0)
        }>
            <p class="empty dim">{t!(i18n, nav_no_directs)}</p>
        </Show>
    }
}

/// The bit each section of the conversations takes in the preferences when
/// it is folded away.
const SECTION_CHANNELS: u8 = 1;
const SECTION_ROOMS: u8 = 2;
const SECTION_DIRECTS: u8 = 4;

/// A section of the conversations, folded away at its heading, as it is
/// kept: folded, what still calls for one stays, the conversations with
/// something unread, each with its count, and the one open, so as not to
/// lose where one is.
#[component]
fn Section(bit: u8, title: Signal<String>, peers: Signal<Vec<Peer>>) -> impl IntoView {
    let live = expect_context::<Live>();
    let folded = move || live.preferences.with(|p| p.sections_folded & bit != 0);
    let fold = move |_| {
        let mut preferences = live.preferences.get_untracked();
        preferences.sections_folded ^= bit;
        live.prefer(preferences);
    };
    let shown = move || {
        let open = (live.page.get() == Page::Chat)
            .then(|| live.open.get())
            .flatten();
        let mut peers = peers.get();
        if folded() {
            live.conversations.with(|c| {
                peers.retain(|peer| {
                    Some(*peer) == open
                        || c.iter()
                            .any(|c| c.peer == *peer && c.unread > 0 && !c.muted)
                });
            });
        }
        peers
    };
    view! {
        <h2 class="folding">
            <button type="button" aria-expanded=move || (!folded()).to_string() on:click=fold>
                <span>{move || title.get()}</span>
                <span class="chevron" class:folded=folded inner_html=CHEVRON></span>
            </button>
        </h2>
        <For each=shown key=|peer| *peer let:peer>
            <PeerItem peer />
        </For>
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
            <span class="dim head-facts">
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
            let i18n = use_i18n();
            let hops = move || live.nodes.with(|n| n.get(&node).and_then(|n| n.hops_away));
            // MeshCore's hops are the way the radio keeps to the node, not how
            // far it heard it: what matters is whether it has one, as what is
            // sent floods to find one when it does not.
            let meshcore = move || live.selected_kind() == Some(RadioKind::Meshcore);
            let class = move || match (meshcore(), hops()) {
                (_, Some(0)) => "mark-node direct",
                (true, Some(_)) => "mark-node routed",
                (true, None) => "mark-node no-route",
                (false, Some(1..=2)) => "mark-node near",
                (false, Some(3..=4)) => "mark-node mid",
                (false, _) => "mark-node far",
            };
            let title = move || {
                meshcore().then(|| match hops() {
                    Some(0) => t_string!(i18n, mark_direct).to_owned(),
                    Some(1) => t_string!(i18n, mark_routed_one).to_owned(),
                    Some(hops) => t_string!(i18n, mark_routed).replace("{n}", &hops.to_string()),
                    None => t_string!(i18n, mark_no_route).to_owned(),
                })
            };
            view! { <span class=class title=title></span> }.into_any()
        }
    };
    let label = move || {
        let title = title(live, peer);
        // The sidebar marks channels already.
        title
            .strip_prefix("# ")
            .map_or_else(|| title.clone(), str::to_owned)
    };
    // What others wrote in it not read yet: none while it is read; grey
    // while it is silenced, which then says so.
    let silenced = move || muted(live, peer);
    let unread = move || {
        let read_now = live.open.get() == Some(peer) && live.reading.get();
        let unread = live
            .conversations
            .with(|c| c.iter().find(|c| c.peer == peer).map_or(0, |c| c.unread));
        (!read_now && unread > 0).then(|| {
            view! { <span class="unread" class:quiet=silenced>{unread_count(unread)}</span> }
        })
    };
    view! {
        <button class=class class:muted=silenced on:click=move |_| live.open_chat(peer)>
            {marker}
            <span class="label">{label}</span>
            {unread}
            {move || silenced().then(|| view! { <span class="muted-mark" inner_html=BELL_OFF></span> })}
        </button>
    }
}

/// Whether a conversation of the selected radio is silenced.
pub fn muted(live: Live, peer: Peer) -> bool {
    live.conversations
        .with(|c| c.iter().any(|c| c.peer == peer && c.muted))
}

/// Silences a conversation of the selected radio, or no longer: every
/// client hears of it, this one too.
pub fn toggle_muted(live: Live, peer: Peer) {
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    let now = untrack(|| muted(live, peer));
    spawn_local(async move {
        let _ = daemon::set_muted(radio, peer, !now).await;
    });
}

/// A bell crossed beside the open conversation's name, while it is
/// silenced; its button in the heading, or its menu, gives it voice again.
#[component]
fn SilencedMark(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        muted(live, peer).then(|| {
            view! {
                <span
                    class="mark"
                    title=move || t_string!(i18n, chat_muted)
                    aria-label=move || t_string!(i18n, chat_muted)
                    inner_html=BELL_OFF
                ></span>
            }
        })
    }
}

/// Silences the open conversation, or no longer: lit while it is.
#[component]
fn MuteButton(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let on = move || muted(live, peer);
    let label = move || {
        if on() {
            t_string!(i18n, chat_unmute)
        } else {
            t_string!(i18n, chat_mute)
        }
    };
    view! {
        <button
            class="icon-button spare"
            class:on=on
            aria-pressed=move || on().to_string()
            title=label
            aria-label=label
            inner_html=BELL_OFF
            on:click=move |_| toggle_muted(live, peer)
        ></button>
    }
}

/// How many unread, as a count shows it: past 99, 99+.
pub fn unread_count(unread: u32) -> String {
    if unread > 99 {
        "99+".to_owned()
    } else {
        unread.to_string()
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
    // Its own page, by its avatar and its name, once its node is known.
    let open_own = move |_| {
        if let Some(num) = num() {
            live.open_node(num);
        }
    };
    let own_open = move || {
        live.page.get() == Page::Node && live.node.get().is_some() && live.node.get() == num()
    };
    view! {
        <footer class="local-foot">
            <button
                class="own-node"
                class:active=own_open
                disabled=move || num().is_none()
                title=move || t_string!(i18n, nav_own_node)
                on:click=open_own
            >
                <span class=avatar_class style=move || badge::style(&badge())>
                    {short}
                </span>
                <span class="who">
                    <b>{move || name().unwrap_or_else(|| t_string!(i18n, nav_my_node).to_owned())}</b>
                    <span class="mono dim">{detail}</span>
                </span>
            </button>
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
        Page::Nodes => view! {
            <h1 class="page-title">{t!(i18n, nodes_title)}</h1>
            <crate::links::AddNodeLink />
        }
        .into_any(),
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
        Page::Packets => view! {
            <h1 class="page-title">{t!(i18n, page_packets)}</h1>
            <div class="actions">
                <crate::packets::PauseButton />
            </div>
        }
        .into_any(),
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
        // Back to its page, whose node it is, and said by its name.
        Page::NodeSettings => match live.node.get() {
            Some(node) => view! {
                <button
                    class="icon-button back"
                    title=move || t_string!(i18n, node_page)
                    aria-label=move || t_string!(i18n, node_page)
                    on:click=move |_| live.go(Page::Node)
                >
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M5 12h14M5 12l6 6M5 12l6 -6"></path>
                    </svg>
                </button>
                <div class="heading">
                    <h1>
                        <span class="title-text">
                            {move || crate::chat::title(live, Peer::Node { node })}
                        </span>
                    </h1>
                    <span class="dim">{t!(i18n, admin_title)}</span>
                </div>
            }
            .into_any(),
            None => ().into_any(),
        },
        Page::Preferences => view! {
            <div class="heading">
                <h1>{t!(i18n, prefs_title)}</h1>
                <span class="dim">{t!(i18n, prefs_text)}</span>
            </div>
        }
        .into_any(),
        Page::Notifications => view! {
            <div class="heading">
                <h1>{t!(i18n, notifications_title)}</h1>
                <span class="dim">{t!(i18n, notifications_text)}</span>
            </div>
        }
        .into_any(),
        Page::Data => view! {
            <div class="heading">
                <h1>{t!(i18n, data_title)}</h1>
                <span class="dim">{t!(i18n, data_text)}</span>
            </div>
        }
        .into_any(),
        Page::AutoReplies => view! {
            <div class="heading">
                <h1>{t!(i18n, replies_title)}</h1>
                <span class="dim">{t!(i18n, replies_text)}</span>
            </div>
            <div class="actions">
                <button
                    class="primary with-icon"
                    title=move || t_string!(i18n, reply_add_title)
                    on:click=move |_| {
                        live.reply_editing.set(crate::auto_replies::ReplyEditing::Adding(false));
                    }
                >
                    <span class="icon" inner_html=PLUS></span>
                    <span class="label">{t!(i18n, replies_add)}</span>
                </button>
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
        Peer::Channel { channel } => {
            // A MeshCore channel's region: said under its name, beside its
            // slot, and picked in its button for good.
            let regions = crate::meshcore_regions::ChannelRegions::of(channel);
            let meshcore = move || live.selected_kind() == Some(RadioKind::Meshcore);
            view! {
                <div class="heading">
                    <h1>
                        <span class="title-text">{move || title(live, peer)}</span>
                        <SilencedMark peer />
                    </h1>
                    <span class="dim">
                        {t!(i18n, nav_channel_slot, index = channel)}
                        {move || meshcore().then(|| view! {
                            " · "
                            <crate::meshcore_regions::RegionWords regions />
                        })}
                    </span>
                </div>
                <div class="actions">
                    {move || {
                        meshcore()
                            .then(|| view! { <crate::meshcore_regions::ChannelRegion channel regions /> })
                    }}
                    <button
                        class="icon-button spare"
                        title=move || forget_label(live, i18n, peer)
                        aria-label=move || forget_label(live, i18n, peer)
                        inner_html=TRASH
                        on:click=move |_| forget_conversation(live, i18n, peer)
                    ></button>
                    <MuteButton peer />
                    <HeadingMore can_history=false can_remove=false mute=peer />
                </div>
            }
            .into_any()
        }
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
                    // Its role by name; its code beside, where there is room,
                    // but MeshCore's, whose kinds are named alone.
                    let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
                    if let Some(role) = known.role.clone() {
                        let name = crate::names::role_name(i18n.get_locale(), &role);
                        parts.push(if meshcore {
                            name.into_any()
                        } else {
                            view! {
                                {name}
                                <span class="code">" (" {role} ")"</span>
                            }
                            .into_any()
                        });
                    }
                    if let Some(hardware) = known.hw_model.as_deref().filter(|h| !h.is_empty()) {
                        parts.push(model(hardware).into_any());
                    }
                } else {
                    if let Some(reach) = reach_words(i18n, &known, local()) {
                        parts.push(reach.into_any());
                    }
                    if live.page.get() == Page::Chat && live.is_room(node) {
                        // First, as what matters most in a room, and what a
                        // phone has room for.
                        parts.insert(0, view! { <crate::rooms::AccessWords node /> }.into_any());
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
                if badge::is_emoji(&short_name(live, node)) {
                    "node-avatar emoji"
                } else {
                    "node-avatar"
                }
            };
            let avatar_style = move || avatar_style(live, node, &node_name(live, node));
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
                // Its own node takes no messages from it: its settings instead.
                let settings = (page == Page::Node && local()).then(|| {
                    view! {
                        <button
                            class="with-icon"
                            title=move || t_string!(i18n, nav_radio_settings)
                            on:click=move |_| live.go(Page::RadioSettings)
                        >
                            <span class="icon" inner_html=GEAR></span>
                            <span class="label">{t!(i18n, own_settings)}</span>
                        </button>
                    }
                });
                // Written to only once a MeshCore radio keeps it.
                let unkept = live.selected_kind() == Some(RadioKind::Meshcore)
                    && known().is_some_and(|n| n.on_radio == Some(false));
                let chat =
                    (page == Page::Node && !local() && !unkept && live.talks(node)).then(|| {
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
                // Forgetting it: in a drawer, which says how much.
                let remove = (page == Page::Node && !local()).then(|| {
                    view! {
                        <button
                            class="icon-button spare"
                            title=move || t_string!(i18n, remove_title)
                            aria-label=move || t_string!(i18n, remove_title)
                            inner_html=TRASH
                            on:click=move |_| live.removing.set(true)
                        ></button>
                    }
                });
                let history = (page == Page::Node && !local()).then(|| {
                    view! {
                        <button
                            class="icon-button spare"
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
                // Another Meshtastic node's settings, as its admin.
                let can_admin = page == Page::Node
                    && !local()
                    && live.selected_kind() == Some(RadioKind::Meshtastic);
                let admin = can_admin.then(|| {
                    view! {
                        <button
                            class="icon-button spare"
                            title=move || t_string!(i18n, admin_title)
                            aria-label=move || t_string!(i18n, admin_title)
                            inner_html=GEAR
                            on:click=move |_| live.go(Page::NodeSettings)
                        ></button>
                    }
                });
                // Silencing its conversation, from it; and forgetting it.
                let mute = (page == Page::Chat).then(|| view! { <MuteButton peer /> });
                let forget = (page == Page::Chat).then(|| {
                    let label = move || forget_label(live, i18n, peer);
                    view! {
                        <button
                            class="icon-button spare"
                            title=label
                            aria-label=label
                            inner_html=TRASH
                            on:click=move |_| forget_conversation(live, i18n, peer)
                        ></button>
                    }
                });
                // Where there is no room for them, those in a menu.
                let can_remove = remove.is_some();
                let can_history = history.is_some();
                let muting = (page == Page::Chat).then_some(peer);
                let more = (can_remove || can_history || muting.is_some()).then(|| match muting {
                    Some(mute) => view! { <HeadingMore can_history can_remove mute /> }.into_any(),
                    None => view! { <HeadingMore can_history can_remove can_admin /> }.into_any(),
                });
                // The three dots last, against the edge, as menus go.
                view! { {remove} {history} {admin} {forget} {mute} {profile} {settings} {chat} {more} }
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
                <span class=class style=avatar_style>{move || badge::fit(&short_name(live, node))}</span>
                <div class="heading">
                    <h1>
                        <span class="title-text">{move || title(live, peer)}</span>
                        {star}
                        {muted}
                        {move || (live.page.get() == Page::Chat).then(|| view! { <SilencedMark peer /> })}
                    </h1>
                    <span class="dim">{detail}</span>
                </div>
                <div class="actions">
                    {move || (live.page.get() == Page::Chat && live.is_room(node))
                        .then(|| view! {
                            <crate::rooms::ForgetPassword node />
                            <crate::rooms::LogInAgain node />
                        })}
                    {action}
                </div>
            }
            .into_any()
        }
    }
}

/// Forgetting a conversation, in words: a channel is emptied, staying on
/// the radio; a conversation with a node goes.
fn forget_label(
    live: Live,
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    peer: Peer,
) -> &'static str {
    match peer {
        Peer::Channel { .. } => t_string!(i18n, chat_forget_channel),
        // A room stays among the radio's, emptied.
        Peer::Node { node } if live.is_room(node) => t_string!(i18n, chat_forget_room),
        Peer::Node { .. } => t_string!(i18n, chat_forget_node),
    }
}

/// Forgets a conversation's messages in meshctl, once confirmed: they are
/// gone from what meshctl keeps, not from the mesh.
fn forget_conversation(
    live: Live,
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    peer: Peer,
) {
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    let asked = match peer {
        Peer::Channel { .. } => t_string!(i18n, chat_forget_channel_confirm),
        Peer::Node { node } if live.is_room(node) => t_string!(i18n, chat_forget_room_confirm),
        Peer::Node { .. } => t_string!(i18n, chat_forget_node_confirm),
    };
    let sure = web_sys::window()
        .and_then(|w| w.confirm_with_message(asked).ok())
        .unwrap_or(false);
    if sure {
        spawn_local(async move {
            let _ = crate::daemon::forget_conversation(radio, peer).await;
        });
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

/// A node's page's lesser actions, where the heading has no room for them
/// as icons: behind three dots, each by its icon and its words. Closed by
/// a click anywhere else, or by choosing.
#[component]
fn HeadingMore(
    can_history: bool,
    can_remove: bool,
    /// Whether the node's settings can be read from it, as its admin.
    #[prop(optional)]
    can_admin: bool,
    /// The conversation to silence from it, if any.
    #[prop(optional)]
    mute: Option<Peer>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let closing = window_event_listener(leptos::ev::click, move |_| open.set(false));
    on_cleanup(move || closing.remove());
    let choose = move |what: RwSignal<bool>| {
        move |_| {
            open.set(false);
            what.set(true);
        }
    };
    view! {
        <span class="more-actions">
            <button
                class="icon-button"
                class:on=move || open.get()
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                title=move || t_string!(i18n, heading_more)
                aria-label=move || t_string!(i18n, heading_more)
                inner_html=DOTS_VERTICAL
                on:click=move |ev| {
                    ev.stop_propagation();
                    open.update(|o| *o = !*o);
                }
            ></button>
            <Show when=move || open.get()>
                <div class="heading-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                    {mute
                        .map(|peer| {
                            view! {
                                <button
                                    role="menuitem"
                                    on:click=move |_| {
                                        open.set(false);
                                        toggle_muted(live, peer);
                                    }
                                >
                                    <span class="icon" inner_html=BELL_OFF></span>
                                    {move || {
                                        if muted(live, peer) {
                                            t_string!(i18n, chat_unmute)
                                        } else {
                                            t_string!(i18n, chat_mute)
                                        }
                                    }}
                                </button>
                            }
                        })}
                    {mute
                        .map(|peer| {
                            view! {
                                <button
                                    role="menuitem"
                                    class="danger-item"
                                    on:click=move |_| {
                                        open.set(false);
                                        forget_conversation(live, i18n, peer);
                                    }
                                >
                                    <span class="icon" inner_html=TRASH></span>
                                    {move || forget_label(live, i18n, peer)}
                                </button>
                            }
                        })}
                    {can_admin
                        .then(|| {
                            view! {
                                <button role="menuitem" on:click=move |_| {
                                    open.set(false);
                                    live.go(Page::NodeSettings);
                                }>
                                    <span class="icon" inner_html=GEAR></span>
                                    {t!(i18n, admin_title)}
                                </button>
                            }
                        })}
                    {can_history
                        .then(|| {
                            view! {
                                <button role="menuitem" on:click=choose(live.history)>
                                    <span class="icon" inner_html=HISTORY></span>
                                    {t!(i18n, requests_title)}
                                </button>
                            }
                        })}
                    {can_remove
                        .then(|| {
                            view! {
                                <button role="menuitem" class="danger-item" on:click=choose(live.removing)>
                                    <span class="icon" inner_html=TRASH></span>
                                    {t!(i18n, remove_title)}
                                </button>
                            }
                        })}
                </div>
            </Show>
        </span>
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

/// Whether a request was not sent as the login it went first with failed:
/// the node did not answer it, or said no.
fn login_failure(failure: &Failure) -> bool {
    matches!(
        failure,
        Failure::Rejected(
            meshctl_api::ErrorCode::RadioDidNotAnswer
                | meshctl_api::ErrorCode::WrongPassword
                | meshctl_api::ErrorCode::RadioRefused
                | meshctl_api::ErrorCode::NotOnRadio
        )
    )
}
