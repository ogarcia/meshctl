// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Waypoints on the map: what is drawn of each, the button that shows or
//! hides them, and the form that shares or changes one. Only Meshtastic
//! radios have them.

use crate::app::Live;
use crate::chat::{node_name, title};
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::waypoints::{
    DESCRIPTION_BYTES, Fence, FenceArea, NAME_BYTES, NewWaypoint, Waypoint,
};
use meshctl_api::{ErrorCode, LinkState, Peer, RadioId, RadioKind, WaypointEvent};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = bounds)]
    fn map_bounds() -> Option<String>;
}

const PIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map-pin.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");

/// A waypoint's form, open: to share one where the map was clicked, or to
/// change one (`id`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Form {
    pub id: Option<u32>,
    pub latitude: f64,
    pub longitude: f64,
}

impl Form {
    /// To share one there.
    pub fn at(latitude: f64, longitude: f64) -> Self {
        Self {
            id: None,
            latitude,
            longitude,
        }
    }
}

/// The radio's own node number, if it is known.
fn local(live: Live) -> Option<u32> {
    live.status
        .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
}

/// Whether the selected radio can share a waypoint now: a Meshtastic one,
/// connected and up to date.
fn can_share(live: Live) -> bool {
    live.selected_radio()
        .is_some_and(|r| r.kind == RadioKind::Meshtastic && r.link == LinkState::Ready)
}

/// Whether the selected radio's map shows its waypoints: any radio's, as
/// one of meshctl's alone may be on any.
fn shown(live: Live) -> bool {
    live.selected_radio().is_some_and(|r| r.map_waypoints)
}

/// What the map shows of a waypoint, in the interface's language.
#[derive(Debug, Serialize)]
struct Drawn {
    id: u32,
    lat: f64,
    lon: f64,
    icon: String,
    name: String,
    description: String,
    facts: Vec<Fact>,
    locked: bool,
    /// Whether it is meshctl's alone.
    local: bool,
    /// Whether it can be changed, and deleted for everyone, by this radio,
    /// now: one of meshctl's alone, always.
    editable: bool,
    fence: Option<DrawnFence>,
    /// For one with a fence that tells of anything: whether meshctl tells
    /// of it; none for one without.
    watched: Option<bool>,
    /// The latest nodes gone in or out of its fence, in words.
    crossings: Vec<Crossed>,
}

/// A node gone in or out of a fence, as its popup lists it.
#[derive(Debug, Serialize)]
struct Crossed {
    who: String,
    entered: bool,
    /// What it did: went in, or out.
    said: String,
    /// How long ago.
    ago: String,
}

/// A fence as the map draws it: a circle, in metres, a box, or both.
#[derive(Debug, Serialize)]
struct DrawnFence {
    radius: Option<u32>,
    area: Option<[[f64; 2]; 2]>,
}

/// A distance, in short: metres, or kilometres from one.
fn distance(metres: u32) -> String {
    if metres < 1_000 {
        format!("{metres} m")
    } else if metres.is_multiple_of(1_000) {
        format!("{} km", metres / 1_000)
    } else {
        format!("{:.1} km", f64::from(metres) / 1_000.0)
    }
}

/// What a fence is, in words, and what is told of it.
fn fence_words(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, fence: &Fence) -> String {
    let mut shapes = Vec::new();
    if let Some(radius) = fence.radius.filter(|r| *r > 0) {
        shapes.push(t_string!(i18n, waypoint_fence_circle_of).replace("{r}", &distance(radius)));
    }
    if fence.area.is_some() {
        shapes.push(t_string!(i18n, waypoint_fence_box).to_owned());
    }
    let mut told = Vec::new();
    if fence.notify_on_enter {
        told.push(t_string!(i18n, waypoint_fence_in));
    }
    if fence.notify_on_exit {
        told.push(t_string!(i18n, waypoint_fence_out));
    }
    let shapes = shapes.join(" + ");
    if told.is_empty() {
        shapes
    } else {
        format!("{shapes} · {}", told.join(", "))
    }
}

#[derive(Debug, Serialize)]
struct Fact {
    icon: &'static str,
    label: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct Labels {
    add: String,
    edit: String,
    #[serde(rename = "deleteAll")]
    delete_all: String,
    #[serde(rename = "deleteHere")]
    delete_here: String,
    locked: String,
    watch: String,
    unwatch: String,
    crossings: String,
    /// What a waypoint is called in the list of what overlaps.
    waypoint: String,
    /// How many there are, nodes and waypoints, where a click fell.
    here: String,
    #[serde(rename = "localTag")]
    local_tag: String,
    delete: String,
}

/// What the map draws of the waypoints, as `map.js` takes it.
#[derive(Debug, Serialize)]
pub struct Drawing {
    labels: Labels,
    /// Whether the map's menu offers to share one.
    add: bool,
    waypoints: Vec<Drawn>,
}

/// How long until a time to come, in short: as long ago, the other way.
fn until(now: i64, at: i64) -> String {
    ago(
        u32::try_from(at).unwrap_or(u32::MAX),
        u32::try_from(now).unwrap_or(0),
    )
}

/// What the map draws of the selected radio's waypoints: none while its
/// map leaves them out.
pub fn drawing(live: Live, i18n: leptos_i18n::I18nContext<crate::i18n::Locale>) -> Drawing {
    let labels = Labels {
        add: t_string!(i18n, waypoint_add).to_owned(),
        edit: t_string!(i18n, waypoint_edit).to_owned(),
        delete_all: t_string!(i18n, waypoint_delete_all).to_owned(),
        delete_here: t_string!(i18n, waypoint_delete_here).to_owned(),
        locked: t_string!(i18n, waypoint_locked).to_owned(),
        watch: t_string!(i18n, waypoint_watch).to_owned(),
        unwatch: t_string!(i18n, waypoint_unwatch).to_owned(),
        crossings: t_string!(i18n, waypoint_crossings).to_owned(),
        waypoint: t_string!(i18n, waypoint_kind).to_owned(),
        here: t_string!(i18n, map_here_any).to_owned(),
        local_tag: t_string!(i18n, waypoint_local_tag).to_owned(),
        delete: t_string!(i18n, waypoint_delete).to_owned(),
    };
    let sharing = can_share(live);
    // Any radio's map may have one of meshctl's alone.
    let adding = live.selected_radio().is_some();
    if !shown(live) {
        return Drawing {
            labels,
            add: adding,
            waypoints: Vec::new(),
        };
    }
    let now = i64::from(live.now.get());
    let local = local(live);
    let by = |num: u32| {
        if num == 0 || Some(num) == local {
            t_string!(i18n, chat_this_radio).to_owned()
        } else {
            node_name(live, num)
        }
    };
    let waypoints = live.waypoints.with(|waypoints| {
        waypoints
            .iter()
            .filter(|w| w.expire.is_none_or(|at| at > now))
            .map(|w| {
                let mut facts = vec![
                    Fact {
                        icon: "user",
                        label: t_string!(i18n, waypoint_by).to_owned(),
                        value: by(w.created_by),
                    },
                    Fact {
                        icon: "clock",
                        label: t_string!(i18n, waypoint_changed).to_owned(),
                        value: ago(
                            u32::try_from(now).unwrap_or(0),
                            u32::try_from(w.updated_at).unwrap_or(0),
                        ),
                    },
                    Fact {
                        icon: "history",
                        label: t_string!(i18n, waypoint_expires).to_owned(),
                        value: w.expire.map_or_else(
                            || t_string!(i18n, waypoint_never).to_owned(),
                            |at| t_string!(i18n, waypoint_in).replace("{t}", &until(now, at)),
                        ),
                    },
                ];
                if !w.local {
                    facts.push(Fact {
                        icon: "hash",
                        label: t_string!(i18n, waypoint_channel).to_owned(),
                        value: title(live, Peer::Channel { channel: w.channel }),
                    });
                }
                if let Some(fence) = w.fence.filter(Fence::is_some) {
                    facts.push(Fact {
                        icon: "current-location",
                        label: t_string!(i18n, waypoint_fence).to_owned(),
                        value: fence_words(i18n, &fence),
                    });
                }
                if w.updated_by != w.created_by {
                    facts.insert(
                        1,
                        Fact {
                            icon: "user",
                            label: t_string!(i18n, waypoint_changed_by).to_owned(),
                            value: by(w.updated_by),
                        },
                    );
                }
                Drawn {
                    id: w.id,
                    lat: w.latitude,
                    lon: w.longitude,
                    icon: w.icon.clone(),
                    name: w.name.clone(),
                    description: w.description.clone(),
                    facts,
                    locked: w.locked_to.is_some(),
                    local: w.local,
                    editable: w.local || sharing && w.locked_to.is_none_or(|to| Some(to) == local),
                    crossings: w
                        .crossings
                        .iter()
                        .map(|c| Crossed {
                            who: by(c.node),
                            entered: c.entered,
                            said: if c.entered {
                                t_string!(i18n, waypoint_crossed_in)
                            } else {
                                t_string!(i18n, waypoint_crossed_out)
                            }
                            .to_owned(),
                            ago: t_string!(i18n, waypoint_ago).replace(
                                "{t}",
                                &ago(
                                    u32::try_from(now).unwrap_or(0),
                                    u32::try_from(c.at).unwrap_or(0),
                                ),
                            ),
                        })
                        .collect(),
                    watched: w
                        .fence
                        .filter(|f| f.is_some() && (f.notify_on_enter || f.notify_on_exit))
                        .map(|_| w.watched),
                    fence: w.fence.filter(Fence::is_some).map(|f| DrawnFence {
                        radius: f.radius.filter(|r| *r > 0),
                        area: f.area.map(|a| [[a.south, a.west], [a.north, a.east]]),
                    }),
                }
            })
            .collect()
    });
    Drawing {
        labels,
        add: adding,
        waypoints,
    }
}

/// What happened to a waypoint, told over the panel for a while.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    key: u32,
    radio: RadioId,
    waypoint: u32,
    who: String,
    icon: String,
    name: String,
    event: WaypointEvent,
}

/// How long a notice stays, in milliseconds.
const NOTICE_MS: u32 = 15_000;

/// Tells of what happened to a waypoint over the panel, for a while.
pub fn tell(
    live: Live,
    radio: RadioId,
    waypoint: u32,
    who: String,
    icon: String,
    name: String,
    event: WaypointEvent,
) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let key = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    live.notices.update(|notices| {
        notices.push(Notice {
            key,
            radio,
            waypoint,
            who,
            icon,
            name,
            event,
        });
        // A few at once, the newest.
        let extra = notices.len().saturating_sub(3);
        notices.drain(..extra);
    });
    spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(NOTICE_MS).await;
        live.notices
            .try_update(|notices| notices.retain(|n| n.key != key));
    });
}

/// What happened to waypoints, over the panel: each with the way to see it
/// on the map, and to put it away.
#[component]
pub fn Notices() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let many_radios = move || {
        live.radios
            .with(|r| r.as_ref().is_some_and(|r| r.len() > 1))
    };
    let radio_name = move |radio: RadioId| {
        live.radios.with(|r| {
            r.as_ref()
                .and_then(|r| r.iter().find(|r| r.id == radio))
                .map(|r| r.name.clone())
                .unwrap_or_default()
        })
    };
    view! {
        <div class="notices" role="status" aria-live="polite">
            <For
                each=move || live.notices.get()
                key=|notice| notice.key
                children=move |notice| {
                    let said = match notice.event {
                        WaypointEvent::Shared => t_string!(i18n, notice_shared),
                        WaypointEvent::Entered => t_string!(i18n, notice_entered),
                        WaypointEvent::Left => t_string!(i18n, notice_left),
                    };
                    let what = if notice.name.is_empty() {
                        String::new()
                    } else {
                        format!("{} {}", notice.icon, notice.name)
                    };
                    let (radio, waypoint, key) = (notice.radio, notice.waypoint, notice.key);
                    let close_label = move || t_string!(i18n, notice_close);
                    view! {
                        <div class="notice">
                            <span class="notice-text">
                                <strong>{notice.who.clone()}</strong>
                                " "
                                {said}
                                " "
                                <strong>{what}</strong>
                                {move || {
                                    many_radios()
                                        .then(|| {
                                            view! {
                                                <span class="dim">{format!(" · {}", radio_name(radio))}</span>
                                            }
                                        })
                                }}
                            </span>
                            <button
                                type="button"
                                on:click=move |_| {
                                    if live.selected.get_untracked() != Some(radio) {
                                        live.select(radio);
                                    }
                                    live.show_waypoint(waypoint);
                                    live.notices.update(|n| n.retain(|n| n.key != key));
                                }
                            >
                                {t!(i18n, notice_see)}
                            </button>
                            <button
                                type="button"
                                class="close"
                                title=close_label
                                aria-label=close_label
                                on:click=move |_| live.notices.update(|n| n.retain(|n| n.key != key))
                                inner_html=CLOSE
                            ></button>
                        </div>
                    }
                }
            />
        </div>
    }
}

/// Watches a waypoint's fence, telling of nodes going in or out of it, or
/// no longer.
pub fn watch(live: Live, id: u32, watched: bool) {
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    spawn_local(async move {
        let _ = daemon::watch_waypoint(radio, id, watched).await;
    });
}

/// Opens the form of one of the selected radio's waypoints, to change it.
pub fn edit(live: Live, id: u32) {
    let at = live.waypoints.with_untracked(|waypoints| {
        waypoints
            .iter()
            .find(|w| w.id == id)
            .map(|w| (w.latitude, w.longitude))
    });
    if let Some((latitude, longitude)) = at {
        live.waypoint_form.set(Some(Form {
            id: Some(id),
            latitude,
            longitude,
        }));
    }
}

/// What a failure to share, change or delete a waypoint means, in words.
fn failure_text(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, failure: &Failure) -> String {
    match failure {
        Failure::Rejected(ErrorCode::TooSoon) => t_string!(i18n, waypoint_too_soon),
        Failure::Rejected(ErrorCode::InvalidWaypoint) => t_string!(i18n, waypoint_invalid),
        Failure::Rejected(ErrorCode::WaypointLocked) => t_string!(i18n, waypoint_locked_error),
        Failure::Rejected(ErrorCode::RadioNotConnected) => t_string!(i18n, waypoint_not_connected),
        _ => t_string!(i18n, error_failed),
    }
    .to_owned()
}

/// Deletes one of the selected radio's waypoints, for everyone or here
/// alone, once the person says so.
pub fn delete(
    live: Live,
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    id: u32,
    everyone: bool,
) {
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    let found = live.waypoints.with_untracked(|waypoints| {
        waypoints
            .iter()
            .find(|w| w.id == id)
            .map(|w| (w.name.clone(), w.local))
    });
    let Some((name, local)) = found else {
        return;
    };
    let asked = if local {
        t_string!(i18n, waypoint_delete_local_confirm)
    } else if everyone {
        t_string!(i18n, waypoint_delete_all_confirm)
    } else {
        t_string!(i18n, waypoint_delete_here_confirm)
    }
    .replace("{name}", &name);
    let sure = web_sys::window()
        .and_then(|w| w.confirm_with_message(&asked).ok())
        .unwrap_or(false);
    if !sure {
        return;
    }
    spawn_local(async move {
        if let Err(failure) = daemon::delete_waypoint(radio, id, everyone).await {
            let said = failure_text(i18n, &failure);
            if let Some(w) = web_sys::window() {
                let _ = w.alert_with_message(&said);
            }
        }
    });
}

/// Shows or hides the selected radio's waypoints on its map: under the
/// clock, on a Meshtastic radio.
#[component]
pub fn WaypointsButton() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let any = move || live.selected.get().is_some();
    let on = move || shown(live);
    let said = move || {
        if on() {
            t_string!(i18n, waypoints_hide)
        } else {
            t_string!(i18n, waypoints_show)
        }
    };
    let flip = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let shown = !on();
        live.radios.update(|radios| {
            if let Some(r) = radios.iter_mut().flatten().find(|r| r.id == radio) {
                r.map_waypoints = shown;
            }
        });
        spawn_local(async move {
            let _ = daemon::set_map_waypoints(radio, shown).await;
        });
    };
    view! {
        <Show when=any>
            <button
                type="button"
                class="map-waypoints-toggle"
                class:on=on
                aria-pressed=move || on().to_string()
                title=said
                aria-label=said
                inner_html=PIN
                on:click=flip
            ></button>
        </Show>
    }
}

/// When a waypoint expires, as the form offers it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expiry {
    Never,
    /// So many seconds from when it is shared.
    In(i64),
    /// As it is, for one changed: when it expires already.
    Kept(i64),
}

const EXPIRIES: [i64; 5] = [3_600, 28_800, 86_400, 259_200, 604_800];

/// The radii a fence's circle is offered with, in metres, as Android's.
const RADII: [u32; 6] = [100, 250, 500, 1_000, 2_000, 5_000];

/// What shape a fence takes in the form.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    None,
    Circle,
    Area,
    /// Both, as one heard may be: kept so.
    Both,
}

/// What the form holds as it is filled in.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    name: String,
    description: String,
    icon: Option<String>,
    expiry: Expiry,
    locked: bool,
    channel: u32,
    shape: Shape,
    radius: u32,
    area: Option<FenceArea>,
    /// meshctl's alone, never sent.
    local: bool,
    notify_on_enter: bool,
    notify_on_exit: bool,
    notify_favorites_only: bool,
}

/// The area the map shows now, if it is there.
fn shown_area() -> Option<FenceArea> {
    let json = map_bounds()?;
    let area: FenceArea = serde_json::from_str(&json).ok()?;
    area.is_valid().then_some(area)
}

impl Draft {
    fn blank() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            icon: None,
            expiry: Expiry::Never,
            locked: false,
            channel: 0,
            shape: Shape::None,
            radius: 250,
            area: None,
            local: false,
            notify_on_enter: true,
            notify_on_exit: false,
            notify_favorites_only: false,
        }
    }

    fn of(waypoint: &Waypoint) -> Self {
        let fence = waypoint.fence.filter(Fence::is_some);
        let radius = fence.and_then(|f| f.radius).filter(|r| *r > 0);
        let area = fence.and_then(|f| f.area);
        Self {
            name: waypoint.name.clone(),
            description: waypoint.description.clone(),
            icon: Some(waypoint.icon.clone()),
            expiry: waypoint.expire.map_or(Expiry::Never, Expiry::Kept),
            locked: waypoint.locked_to.is_some(),
            channel: waypoint.channel,
            local: waypoint.local,
            shape: match (radius, area) {
                (Some(_), Some(_)) => Shape::Both,
                (Some(_), None) => Shape::Circle,
                (None, Some(_)) => Shape::Area,
                (None, None) => Shape::None,
            },
            radius: radius.unwrap_or(250),
            area,
            notify_on_enter: fence.is_some_and(|f| f.notify_on_enter),
            notify_on_exit: fence.is_some_and(|f| f.notify_on_exit),
            notify_favorites_only: fence.is_some_and(|f| f.notify_favorites_only),
        }
    }

    fn fence(&self) -> Option<Fence> {
        let (radius, area) = match self.shape {
            Shape::None => return None,
            Shape::Circle => (Some(self.radius), None),
            Shape::Area => (None, self.area),
            Shape::Both => (Some(self.radius), self.area),
        };
        let fence = Fence {
            radius,
            area,
            notify_on_enter: self.notify_on_enter,
            notify_on_exit: self.notify_on_exit,
            notify_favorites_only: self.notify_favorites_only,
        };
        fence.is_some().then_some(fence)
    }

    fn waypoint(&self, form: Form, now: i64) -> NewWaypoint {
        NewWaypoint {
            latitude: form.latitude,
            longitude: form.longitude,
            name: self.name.trim().to_owned(),
            description: self.description.trim().to_owned(),
            icon: self.icon.clone(),
            expire: match self.expiry {
                Expiry::Never => None,
                Expiry::In(seconds) => Some(now + seconds),
                Expiry::Kept(at) => Some(at),
            },
            locked: self.locked && !self.local,
            fence: self.fence(),
            channel: if self.local { 0 } else { self.channel },
            local: self.local,
        }
    }
}

/// The form that shares a waypoint, or changes one: open while
/// [`Live::waypoint_form`] says.
#[component]
pub fn WaypointForm() -> impl IntoView {
    let live = expect_context::<Live>();
    move || {
        live.waypoint_form
            .get()
            .map(|form| view! { <FormDialog form /> })
    }
}

#[component]
fn FormDialog(form: Form) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let kept = form.id.and_then(|id| {
        live.waypoints
            .with_untracked(|waypoints| waypoints.iter().find(|w| w.id == id).cloned())
    });
    // One the radio cannot share now is meshctl's alone, as one already so.
    let sharing = untrack(|| can_share(live));
    let draft = RwSignal::new(kept.as_ref().map_or_else(
        || Draft {
            local: !sharing,
            ..Draft::blank()
        },
        Draft::of,
    ));
    // One on the mesh stays there: no way to make it meshctl's alone.
    let shared_already = kept.as_ref().is_some_and(|w| !w.local);
    let local = move || draft.with(|d| d.local);
    let was_both = draft.with_untracked(|d| d.shape == Shape::Both);
    let kept_expiry = kept.as_ref().and_then(|w| w.expire);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let emojis = RwSignal::new(false);
    let close = move || live.waypoint_form.set(None);

    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| (c.index, title(live, Peer::Channel { channel: c.index })))
                .collect::<Vec<_>>()
        })
    };
    let expiries = move || {
        let mut options: Vec<(String, String)> = vec![(
            "never".to_owned(),
            t_string!(i18n, waypoint_never).to_owned(),
        )];
        if let Some(at) = kept_expiry {
            let now = i64::from(live.now.get_untracked());
            options.push((
                "kept".to_owned(),
                t_string!(i18n, waypoint_expiry_kept).replace("{t}", &until(now, at)),
            ));
        }
        for seconds in EXPIRIES {
            let label = match seconds {
                3_600 => t_string!(i18n, waypoint_expiry_hour),
                28_800 => t_string!(i18n, waypoint_expiry_hours),
                86_400 => t_string!(i18n, waypoint_expiry_day),
                259_200 => t_string!(i18n, waypoint_expiry_days),
                _ => t_string!(i18n, waypoint_expiry_week),
            };
            options.push((seconds.to_string(), label.to_owned()));
        }
        options
    };
    let expiry_value = move || match draft.with(|d| d.expiry) {
        Expiry::Never => "never".to_owned(),
        Expiry::Kept(_) => "kept".to_owned(),
        Expiry::In(seconds) => seconds.to_string(),
    };
    let pick_expiry = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        let expiry = match value.as_str() {
            "never" => Expiry::Never,
            "kept" => kept_expiry.map_or(Expiry::Never, Expiry::Kept),
            seconds => seconds.parse().map_or(Expiry::Never, Expiry::In),
        };
        draft.update(|d| d.expiry = expiry);
    };

    let shape_value = move || match draft.with(|d| d.shape) {
        Shape::None => "none",
        Shape::Circle => "circle",
        Shape::Area => "area",
        Shape::Both => "both",
    };
    let pick_shape = move |ev: leptos::ev::Event| {
        let shape = match event_target_value(&ev).as_str() {
            "circle" => Shape::Circle,
            "area" => Shape::Area,
            "both" => Shape::Both,
            _ => Shape::None,
        };
        draft.update(|d| {
            d.shape = shape;
            // A box is what the map shows, the first time one is asked for.
            if matches!(shape, Shape::Area | Shape::Both) && d.area.is_none() {
                d.area = shown_area();
            }
        });
    };
    let radii = move || {
        let current = draft.with(|d| d.radius);
        let mut radii = RADII.to_vec();
        if !radii.contains(&current) {
            radii.push(current);
            radii.sort_unstable();
        }
        radii
    };
    let circled = move || draft.with(|d| matches!(d.shape, Shape::Circle | Shape::Both));
    let boxed = move || draft.with(|d| matches!(d.shape, Shape::Area | Shape::Both));
    let fenced = move || draft.with(|d| d.shape != Shape::None);
    let area_words = move || {
        draft.with(|d| {
            d.area.map_or_else(
                || t_string!(i18n, waypoint_fence_area_none).to_owned(),
                |a| {
                    format!(
                        "{:.4}, {:.4} → {:.4}, {:.4}",
                        a.south, a.west, a.north, a.east
                    )
                },
            )
        })
    };
    let check = move |label: &'static str, get: fn(&Draft) -> bool, set: fn(&mut Draft, bool)| {
        view! {
            <label class="toggle">
                <input
                    type="checkbox"
                    prop:checked=move || draft.with(get)
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        draft.update(|d| set(d, on));
                    }
                />
                <span>{label}</span>
            </label>
        }
    };

    let name_bytes = move || draft.with(|d| d.name.trim().len());
    let description_bytes = move || draft.with(|d| d.description.trim().len());
    let name_over = move || name_bytes() > NAME_BYTES;
    let description_over = move || description_bytes() > DESCRIPTION_BYTES;
    let valid = move || {
        draft.with(|d| {
            !d.name.trim().is_empty()
                && d.name.trim().len() <= NAME_BYTES
                && d.description.trim().len() <= DESCRIPTION_BYTES
                && (!matches!(d.shape, Shape::Area | Shape::Both) || d.area.is_some())
        })
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let now = i64::from(live.now.get_untracked());
        let waypoint = draft.with_untracked(|d| d.waypoint(form, now));
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match daemon::send_waypoint(radio, form.id, &waypoint).await {
                Ok(_) => live.waypoint_form.set(None),
                Err(failure) => {
                    error.try_set(Some(failure_text(i18n, &failure)));
                    busy.try_set(false);
                }
            }
        });
    };

    let pick = Callback::new(move |emoji: String| {
        draft.update(|d| d.icon = Some(emoji));
        emojis.set(false);
    });
    let picked = Signal::derive(move || draft.with(|d| d.icon.clone()));
    let icon_shown = move || draft.with(|d| d.icon.clone().unwrap_or_else(|| "📍".to_owned()));
    let heading = move || {
        if form.id.is_some() {
            t_string!(i18n, waypoint_edit_title)
        } else {
            t_string!(i18n, waypoint_new_title)
        }
    };
    let where_ = format!("{:.5}, {:.5}", form.latitude, form.longitude);
    let close_label = move || t_string!(i18n, radio_cancel);

    view! {
        <div class="modal-back">
            <form
                class="card radio-editor modal waypoint-form"
                role="dialog"
                aria-modal="true"
                on:submit=submit
            >
                <header class="editor-head">
                    <span class="who">
                        <h2>{heading}</h2>
                        <span class="dim mono">{where_}</span>
                    </span>
                    <button
                        type="button"
                        class="close"
                        title=close_label
                        aria-label=close_label
                        on:click=move |_| close()
                        inner_html=CLOSE
                    ></button>
                </header>
                <div class="modal-body">
                    <div class="waypoint-name-row">
                        <button
                            type="button"
                            class="waypoint-icon-pick"
                            class:on=move || emojis.get()
                            aria-expanded=move || emojis.get().to_string()
                            title=move || t_string!(i18n, waypoint_icon)
                            aria-label=move || t_string!(i18n, waypoint_icon)
                            on:click=move |_| emojis.update(|e| *e = !*e)
                        >
                            {icon_shown}
                        </button>
                        <label class="field waypoint-name">
                            <span>{t!(i18n, waypoint_name)}</span>
                            <input
                                required
                                autofocus
                                prop:value=move || draft.with(|d| d.name.clone())
                                on:input=move |ev| {
                                    let name = event_target_value(&ev);
                                    draft.update(|d| d.name = name);
                                }
                            />
                        </label>
                        <span class="waypoint-count dim" class:over=name_over>
                            {move || format!("{}/{NAME_BYTES}", name_bytes())}
                        </span>
                    </div>
                    <Show when=move || emojis.get()>
                        <div class="waypoint-emojis">
                            <crate::emoji::EmojiPicker picked pick start=7 />
                        </div>
                    </Show>
                    <label class="field">
                        <span>
                            {t!(i18n, waypoint_description)}
                            <span
                                class="waypoint-count dim"
                                class:over=description_over
                            >
                                {move || format!("{}/{DESCRIPTION_BYTES}", description_bytes())}
                            </span>
                        </span>
                        <textarea
                            rows="2"
                            prop:value=move || draft.with(|d| d.description.clone())
                            on:input=move |ev| {
                                let description = event_target_value(&ev);
                                draft.update(|d| d.description = description);
                            }
                        ></textarea>
                    </label>
                    {(!shared_already)
                        .then(|| {
                            view! {
                                <label class="toggle">
                                    <input
                                        type="checkbox"
                                        disabled=!sharing
                                        prop:checked=local
                                        on:change=move |ev| {
                                            let on = event_target_checked(&ev);
                                            draft.update(|d| d.local = on);
                                        }
                                    />
                                    <span>
                                        {t!(i18n, waypoint_local)}
                                        <span class="dim">
                                            {if sharing {
                                                t!(i18n, waypoint_local_hint).into_any()
                                            } else if untrack(|| live.selected_kind())
                                                == Some(RadioKind::Meshcore)
                                            {
                                                t!(i18n, waypoint_local_meshcore).into_any()
                                            } else {
                                                t!(i18n, waypoint_local_only).into_any()
                                            }}
                                        </span>
                                    </span>
                                </label>
                            }
                        })}
                    <div class="waypoint-pair">
                        <label class="field">
                            <span>{t!(i18n, waypoint_expires)}</span>
                            <select prop:value=expiry_value on:change=pick_expiry>
                                {move || {
                                    expiries()
                                        .into_iter()
                                        .map(|(value, label)| {
                                            let chosen = value == expiry_value();
                                            view! {
                                                <option value=value selected=chosen>
                                                    {label}
                                                </option>
                                            }
                                        })
                                        .collect_view()
                                }}
                            </select>
                        </label>
                        <label class="field" class:gone=local>
                            <span>{t!(i18n, waypoint_channel)}</span>
                            <select
                                prop:value=move || draft.with(|d| d.channel.to_string())
                                on:change=move |ev| {
                                    let channel = event_target_value(&ev).parse().unwrap_or(0);
                                    draft.update(|d| d.channel = channel);
                                }
                            >
                                {move || {
                                    channels()
                                        .into_iter()
                                        .map(|(index, name)| {
                                            let chosen = draft.with(|d| d.channel == index);
                                            view! {
                                                <option value=index.to_string() selected=chosen>
                                                    {name}
                                                </option>
                                            }
                                        })
                                        .collect_view()
                                }}
                            </select>
                        </label>
                    </div>
                    <p class="hint dim" class:gone=local>{t!(i18n, waypoint_channel_hint)}</p>
                    <div class="waypoint-fence">
                        <div class="waypoint-pair">
                            <label class="field">
                                <span>{t!(i18n, waypoint_fence)}</span>
                                <select prop:value=shape_value on:change=pick_shape>
                                    <option value="none" selected=move || shape_value() == "none">
                                        {t!(i18n, waypoint_fence_none)}
                                    </option>
                                    <option value="circle" selected=move || shape_value() == "circle">
                                        {t!(i18n, waypoint_fence_circle)}
                                    </option>
                                    <option value="area" selected=move || shape_value() == "area">
                                        {t!(i18n, waypoint_fence_area)}
                                    </option>
                                    {was_both
                                        .then(|| {
                                            view! {
                                                <option value="both" selected=move || shape_value() == "both">
                                                    {t!(i18n, waypoint_fence_both)}
                                                </option>
                                            }
                                        })}
                                </select>
                            </label>
                            <Show when=circled>
                                <label class="field">
                                    <span>{t!(i18n, waypoint_fence_radius)}</span>
                                    <select
                                        prop:value=move || draft.with(|d| d.radius.to_string())
                                        on:change=move |ev| {
                                            let radius = event_target_value(&ev).parse().unwrap_or(250);
                                            draft.update(|d| d.radius = radius);
                                        }
                                    >
                                        {move || {
                                            radii()
                                                .into_iter()
                                                .map(|radius| {
                                                    let chosen = draft.with(|d| d.radius == radius);
                                                    view! {
                                                        <option value=radius.to_string() selected=chosen>
                                                            {distance(radius)}
                                                        </option>
                                                    }
                                                })
                                                .collect_view()
                                        }}
                                    </select>
                                </label>
                            </Show>
                        </div>
                        <Show when=boxed>
                            <div class="waypoint-area">
                                <span class="mono dim">{area_words}</span>
                                <button
                                    type="button"
                                    on:click=move |_| draft.update(|d| d.area = shown_area())
                                >
                                    {t!(i18n, waypoint_fence_area_now)}
                                </button>
                            </div>
                            <p class="hint dim">{t!(i18n, waypoint_fence_area_hint)}</p>
                        </Show>
                        <Show when=fenced>
                            <div class="waypoint-notify">
                                {check(
                                    t_string!(i18n, waypoint_notify_enter),
                                    |d| d.notify_on_enter,
                                    |d, on| d.notify_on_enter = on,
                                )}
                                {check(
                                    t_string!(i18n, waypoint_notify_exit),
                                    |d| d.notify_on_exit,
                                    |d, on| d.notify_on_exit = on,
                                )}
                                {check(
                                    t_string!(i18n, waypoint_notify_favorites),
                                    |d| d.notify_favorites_only,
                                    |d, on| d.notify_favorites_only = on,
                                )}
                            </div>
                            <p class="hint dim">{t!(i18n, waypoint_notify_hint)}</p>
                        </Show>
                    </div>
                    <label class="toggle" class:gone=local>
                        <input
                            type="checkbox"
                            prop:checked=move || draft.with(|d| d.locked)
                            on:change=move |ev| {
                                let locked = event_target_checked(&ev);
                                draft.update(|d| d.locked = locked);
                            }
                        />
                        <span>
                            {t!(i18n, waypoint_lock)}
                            <span class="dim">{t!(i18n, waypoint_lock_hint)}</span>
                        </span>
                    </label>
                    {move || error.get().map(|said| view! { <p class="error">{said}</p> })}
                </div>
                <div class="buttons">
                    <button type="button" on:click=move |_| close()>
                        {t!(i18n, radio_cancel)}
                    </button>
                    <button
                        type="submit"
                        class="primary"
                        disabled=move || busy.get() || !valid()
                    >
                        {move || {
                            if form.id.is_some() || local() {
                                t_string!(i18n, waypoint_save)
                            } else {
                                t_string!(i18n, waypoint_share)
                            }
                        }}
                    </button>
                </div>
            </form>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::until;

    #[test]
    fn how_long_until() {
        assert_eq!(until(1_000_000, 1_000_000 + 90 * 60), "1 h");
        assert_eq!(until(1_000_000, 1_000_000 + 3 * 86_400), "3 d");
        assert_eq!(until(1_000_000, 1_000_000 + 30), "30 s");
    }
}
