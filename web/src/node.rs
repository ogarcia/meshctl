// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A node's page: how it is doing, what it last told of each kind, each with
//! the request that asks it again, and the routes traced to it.
//!
//! What it is asked for fills the page, not a list of requests: each section
//! says when its data came, whether sent by the node of its own accord or
//! answering, and has the button that asks for it anew.

use crate::app::Live;
use crate::chat::title;
use crate::daemon::{self, Failure, REQUESTS_PAGE, TRACES_PAGE};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::mesh::{ago, awaiting_clock, radius, reach_words, waited};
use crate::traces::TraceCard;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{
    ErrorCode, LinkState, NodeCheck, NodeLatest, Pace, Packet, Peer, PositionSource, RadioKind,
    Request, RequestKind, RequestStatus, SettingsSection, Traceroute, TracerouteStatus,
};
use serde_json::{Map, Value};
use std::time::Duration;

const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
const QR: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/qrcode.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");

/// What the open node's requests go by: the node, and a clock by the second
/// for how long the radio keeps from sending.
#[derive(Clone, Copy)]
struct Asking {
    num: u32,
    clock: RwSignal<u32>,
}

/// What a button of the page sends: a request of a kind, or a traceroute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    Request(RequestKind),
    Trace,
}

impl Asked {
    fn pace(self) -> Option<Pace> {
        match self {
            Self::Request(kind) => kind.pace(),
            Self::Trace => Some(Pace::Traceroute),
        }
    }

    /// Whether the radio sends it encrypted to the node's own key when it
    /// knows it, whatever the channel: not node info, positions nor
    /// traceroutes, which go on the channel always.
    fn private(self) -> bool {
        matches!(
            self,
            Self::Request(
                RequestKind::DeviceTelemetry
                    | RequestKind::EnvironmentTelemetry
                    | RequestKind::PowerTelemetry
                    | RequestKind::LocalStats
                    | RequestKind::Neighbors
            )
        )
    }
}

/// The radio's login to the node of the page, as last read: which of its
/// sections show, as some are its admin's only.
#[derive(Clone, Copy)]
struct NodeLoginState(RwSignal<Option<meshctl_api::NodeLogin>>);

/// A button of the page: what it sends, and where it is, as two may send
/// the same (its node info, over all while its name is not known, and in
/// its section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonId {
    asked: Asked,
    place: &'static str,
}

/// Which button's channel menu is open, if any.
#[derive(Clone, Copy)]
struct OpenMenu(RwSignal<Option<ButtonId>>);

#[component]
pub fn NodeView() -> impl IntoView {
    let live = expect_context::<Live>();
    move || live.node.get().map(|num| view! { <NodePage num /> })
}

#[component]
fn NodePage(num: u32) -> impl IntoView {
    let live = expect_context::<Live>();
    let clock = RwSignal::new(crate::app::now());
    if let Ok(ticking) =
        set_interval_with_handle(move || clock.set(crate::app::now()), Duration::from_secs(1))
    {
        on_cleanup(move || ticking.clear());
    }
    provide_context(Asking { num, clock });
    provide_context(NodeLoginState(RwSignal::new(None)));
    // One channel menu open at a time, closed by a click anywhere else.
    let menu = RwSignal::new(None::<ButtonId>);
    provide_context(OpenMenu(menu));
    let closing = window_event_listener(leptos::ev::click, move |_| menu.set(None));
    on_cleanup(move || closing.remove());
    let drawer = RwSignal::new(false);
    let latest = move || live.latest.get().unwrap_or_default();
    let is_own = Memo::new(move |_| own(live, num));
    // The page is drawn again for another radio, and so is read once.
    let meshcore = untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore);
    // A MeshCore sensor keeps its readings over time.
    let senses = meshcore
        && untrack(|| {
            live.nodes
                .with(|nodes| nodes.get(&num).and_then(|n| n.role.clone()))
        })
        .is_some_and(|role| role == "SENSOR");
    // A MeshCore repeater or room says which regions it repeats on.
    let serves = meshcore
        && untrack(|| {
            live.nodes
                .with(|nodes| nodes.get(&num).and_then(|n| n.role.clone()))
        })
        .is_some_and(|role| role == "REPEATER" || role == "ROOM");
    // And with a sensor, they are asked what they tell, logged in to.
    let takes_login = meshcore
        && untrack(|| {
            live.nodes
                .with(|nodes| nodes.get(&num).and_then(|n| n.role.clone()))
        })
        .is_some_and(|role| matches!(role.as_str(), "REPEATER" | "ROOM" | "SENSOR"));
    // Two columns, headed by what is looked at most: where it is, and the
    // routes to it: what comes of its own accord on the left, what is asked
    // for on the right, the least likely to answer last.
    let heard = move || {
        // A MeshCore repeater, room or sensor: what is asked of it, logged
        // in to, as it tells nothing of its own accord.
        if takes_login {
            return view! {
                <div class="node-column">
                    {move || {
                        let latest = latest();
                        view! { <PositionSection at=latest.position.as_ref().map(|p| p.at) /> }
                    }}
                    {serves.then(|| view! { <RemoteStatus /> })}
                    <RemoteTelemetry />
                    {senses.then(|| view! { <RemoteSeries /> })}
                    {serves.then(|| view! { <RemoteNeighbors /> })}
                </div>
                <div class="node-column">
                    <MeshcorePath />
                    <RemotePing />
                    <Traces drawer />
                    {serves.then(|| view! { <Regions /> })}
                    <RemoteOwner />
                    <NodeAccessSection />
                    <AccessListSection />
                    <ConsoleSection />
                </div>
            }
            .into_any();
        }
        // A MeshCore companion: where it is when its adverts say, its route
        // and who it is; none of Meshtastic's telemetry, which it never
        // sends, nor anything to ask of it but its route.
        if meshcore {
            return view! {
                <div class="node-column">
                    {move || {
                        let latest = latest();
                        let placed = latest.position.is_some()
                            || live.nodes.with(|n| n.get(&num).is_some_and(|n| n.position.is_some()));
                        placed.then(|| view! {
                            <PositionSection at=latest.position.as_ref().map(|p| p.at) />
                        })
                    }}
                    <MeshcorePath />
                </div>
                <div class="node-column">
                    {move || {
                        let latest = latest();
                        view! { <Identity at=latest.node_info.as_ref().map(|p| p.at) /> }
                    }}
                </div>
            }
            .into_any();
        }
        view! {
            <div class="node-column">
                {move || {
                    let latest = latest();
                    view! {
                        <PositionSection at=latest.position.as_ref().map(|p| p.at) />
                        <Device packet=latest.device.clone() />
                        <Stats packet=latest.local_stats.clone() />
                    }
                }}
            </div>
            <div class="node-column">
                <Traces drawer />
                {move || {
                    let latest = latest();
                    view! {
                        <Identity at=latest.node_info.as_ref().map(|p| p.at) />
                        <Sensors latest />
                    }
                }}
            </div>
        }
        .into_any()
    };
    // The radio's own: how meshctl reaches it, the radio itself, where it
    // is and its settings on the left; how it is, how it hears the mesh and
    // its sensors on the right, as long as each other. Nothing asked of it,
    // as it tells it all of its own accord.
    let mine = move || {
        view! {
            <Link />
            <Radio />
            {move || {
                let latest = latest();
                // Where it is, if it says: a radio with no position has no
                // place for it here.
                let placed = latest.position.is_some()
                    || live
                        .nodes
                        .with(|nodes| nodes.get(&num).is_some_and(|n| n.position.is_some()));
                // A MeshCore radio's figures are asked of it, not heard.
                let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
                view! {
                    {placed.then(|| view! {
                        <PositionSection at=latest.position.as_ref().map(|p| p.at) />
                    })}
                    {if meshcore {
                        view! { <MeshcoreStatus /> }.into_any()
                    } else {
                        view! {
                            <Device packet=latest.device.clone() />
                            <Stats packet=latest.local_stats.clone() />
                            <Sensors latest own=true />
                        }
                        .into_any()
                    }}
                }
            }}
            <Shortcuts />
            {meshcore.then(|| view! { <Advert /> <ConsoleSection own=true /> })}
            <Share />
        }
        .into_any()
    };
    view! {
        <div class="node-page" class:own-page=is_own>
            // Of a node heard, that it can be read but not asked; of the
            // radio's own, nothing is asked anyway.
            {move || view! { <crate::offline::OfflineBanner node=!is_own.get() /> }}
            <Checks />
            <Tiles num />
            <div class="node-sections" class:balanced=is_own>
                {move || if is_own.get() { mine() } else { heard() }}
            </div>
            // Eye-catching, but long: below what is looked at most. Of a
            // MeshCore node but the radio's own, there is none to chart.
            {move || {
                (!meshcore || is_own.get())
                    .then(|| view! { <crate::charts::Telemetry num own=is_own.get() /> })
            }}
            {move || {
                (!is_own.get()).then(|| view! {
                    <TracesDrawer open=drawer />
                    <RequestsDrawer />
                    <RemoveDrawer />
                })
            }}
            <Problem />
        </div>
    }
}

/// Why the last request or traceroute was not sent, over the page for a
/// while, so that nothing moves for it.
#[component]
fn Problem() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    Effect::new(move |_| {
        if live.ask_error.get().is_some() {
            set_timeout(move || live.ask_error.set(None), PROBLEM_SHOWN);
        }
    });
    move || {
        live.ask_error.get().map(|failure| {
            let text = match failure {
                Failure::Rejected(ErrorCode::TooSoon) => t!(i18n, ask_too_soon).into_any(),
                Failure::Rejected(ErrorCode::InvalidChannel) => {
                    t!(i18n, ask_invalid_channel).into_any()
                }
                Failure::Rejected(ErrorCode::RadioNotConnected) => {
                    t!(i18n, node_not_connected).into_any()
                }
                Failure::Rejected(ErrorCode::NoPath) => t!(i18n, ask_no_path).into_any(),
                // The login it goes first with.
                Failure::Rejected(ErrorCode::RadioDidNotAnswer) => {
                    t!(i18n, ask_login_no_answer).into_any()
                }
                Failure::Rejected(ErrorCode::WrongPassword | ErrorCode::RadioRefused) => {
                    t!(i18n, ask_login_refused).into_any()
                }
                _ => t!(i18n, error_failed).into_any(),
            };
            view! { <div class="toast" role="status">{text}</div> }
        })
    }
}

/// How long a request's problem stays over the page.
const PROBLEM_SHOWN: Duration = Duration::from_secs(6);

fn connected(live: Live) -> bool {
    live.status
        .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
}

/// Whether a node is the selected radio's own: it is asked for nothing, as
/// what it tells comes straight from the radio.
fn own(live: Live, num: u32) -> bool {
    live.status
        .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
        == Some(num)
}

/// Whether the selected radio's owner is a licensed radio amateur: then it
/// sends nothing encrypted, and what goes to a node alone goes on the
/// channel.
fn licensed(live: Live) -> bool {
    let local = live
        .status
        .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    local.is_some_and(|local| {
        live.nodes
            .with(|nodes| nodes.get(&local).is_some_and(|n| n.licensed))
    })
}

/// Whether what goes to a node alone cannot be sent to it yet: the radio
/// encrypts it to the node's public key, and refuses to without it, which
/// its node info brings.
fn needs_key(live: Live, num: u32) -> bool {
    !licensed(live)
        && live
            .nodes
            .with(|nodes| nodes.get(&num).is_some_and(|n| n.public_key.is_none()))
}

/// Whether the selected radio is a Meshtastic one, which asks a node with
/// no name for its node info.
fn supported(live: Live) -> bool {
    live.selected_kind() != Some(RadioKind::Meshcore)
}

/// What only MeshCore nodes are asked, and on no channel.
fn meshcore_kind(kind: RequestKind) -> bool {
    matches!(
        kind,
        RequestKind::Regions
            | RequestKind::Status
            | RequestKind::Telemetry
            | RequestKind::Owner
            | RequestKind::AccessList
            | RequestKind::Ping
            | RequestKind::Route
            | RequestKind::Series
    )
}

/// Whether the selected radio asks nodes for this: a MeshCore radio only
/// what MeshCore nodes answer, its neighbours too, and the other way round.
fn can_ask(live: Live, asked: Asked, num: u32) -> bool {
    let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
    match asked {
        Asked::Request(kind) if meshcore => meshcore_kind(kind) || kind == RequestKind::Neighbors,
        Asked::Request(kind) => !meshcore_kind(kind),
        Asked::Trace => !meshcore || meshcore_traceable(live, num),
    }
}

/// Whether a MeshCore node can be traced: a repeater, a room or a sensor,
/// which pass a trace on, the last two when set to, that the radio keeps a
/// path of a hop at least to; one heard straight is pinged instead.
fn meshcore_traceable(live: Live, num: u32) -> bool {
    live.nodes.with(|nodes| {
        nodes.get(&num).is_some_and(|n| {
            matches!(n.role.as_deref(), Some("REPEATER" | "ROOM" | "SENSOR"))
                && n.hops_away.is_some_and(|hops| hops > 0)
        })
    })
}

/// The object a packet's payload carries under a key, as telemetry carries
/// each kind of it; or, with no key, the payload itself.
fn object(packet: Option<&Packet>, key: Option<&str>) -> Option<Map<String, Value>> {
    let payload = packet?.payload.as_ref()?;
    let value = match key {
        Some(key) => payload.get(key)?,
        None => payload,
    };
    value.as_object().cloned()
}

fn number(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key)?.as_f64()
}

/// How long, in the two largest units that say it: `3 d 4 h`, `5 h 12 min`.
fn duration(seconds: u64) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

/// An SNR, coloured as how well it was heard: as a trace's hops are.
pub fn snr_view(snr: f32) -> impl IntoView {
    let quality = crate::traces::quality(Some(snr));
    view! { <span class=format!("snr {quality}")>{format!("{snr:.1} dB")}</span> }
}

/// A fact of a section: what it is, and its value.
fn fact(label: impl IntoView, value: impl IntoView) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd>{value}</dd>
        </div>
    }
}

/// A fact too long to share its row, as a key.
/// What a node says of itself, as it last said it, and when.
fn status_fact(live: Live, node: &meshctl_api::Node) -> Option<impl IntoView + use<>> {
    let i18n = use_i18n();
    let status = node.status.clone()?;
    let at = node.status_at;
    Some(wide_fact(
        t!(i18n, node_status),
        view! {
            <span class="node-status">"«" {status} "»"</span>
            {at.map(|at| view! {
                <span class="dim">
                    " · "
                    {move || {
                        t_string!(i18n, node_ago).replace("{ago}", &crate::mesh::ago(live.now.get(), at))
                    }}
                </span>
            })}
        },
    ))
}

fn wide_fact(label: impl IntoView, value: impl IntoView) -> impl IntoView {
    view! {
        <div class="wide">
            <dt>{label}</dt>
            <dd>{value}</dd>
        </div>
    }
}

/// What is amiss with the node, over all the rest; and, before it, why it
/// may not be asked for anything, or may not answer.
#[component]
fn Checks() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    move || {
        let checks = live.checks.get();
        let companion = !supported(live)
            && !live.nodes.with(|nodes| {
                nodes
                    .get(&asking.num)
                    .and_then(|n| n.role.as_deref())
                    .is_some_and(|role| matches!(role, "REPEATER" | "ROOM" | "SENSOR"))
            });
        let note = if companion {
            Some(t!(i18n, meshcore_companion).into_any())
        } else if !connected(live) {
            // Said over the page, as on every other.
            None
        } else if live.nodes.with(|nodes| {
            nodes
                .get(&asking.num)
                .is_some_and(|n| n.on_radio == Some(false))
        }) {
            Some(t!(i18n, node_kept_hint).into_any())
        } else if live
            .nodes
            .with(|nodes| nodes.get(&asking.num).is_some_and(|n| n.via_mqtt))
        {
            Some(t!(i18n, node_mqtt_hint).into_any())
        } else {
            None
        };
        // Its name, or its key, not known yet: what asks for them, first of
        // all, as without its key it cannot be asked for its telemetry.
        let nameless = live.nodes.with(|nodes| {
            nodes
                .get(&asking.num)
                .is_some_and(|n| n.long_name.is_none())
        });
        let unknown = (supported(live)
            && !own(live, asking.num)
            && (nameless || needs_key(live, asking.num)))
        .then(|| {
            let text = if nameless {
                t!(i18n, node_unknown).into_any()
            } else {
                t!(i18n, node_no_key).into_any()
            };
            view! {
                <li class="info ask-row">
                    <span>{text}</span>
                    <AskButton asked=Asked::Request(RequestKind::NodeInfo) place="top" />
                </li>
            }
        });
        (note.is_some() || unknown.is_some() || !checks.is_empty()).then(|| {
            let note = note.map(|note| view! { <li class="info">{note}</li> });
            let items = checks
                .into_iter()
                .map(|check| {
                    let (heading, text) = match check {
                        NodeCheck::PublicKeyShared { nodes } => {
                            let names = nodes
                                .iter()
                                .map(|node| title(live, Peer::Node { node: *node }))
                                .collect::<Vec<_>>()
                                .join(", ");
                            (
                                t_string!(i18n, check_shared_title).to_owned(),
                                t_string!(i18n, check_shared_text).replace("{nodes}", &names),
                            )
                        }
                        NodeCheck::PublicKeyConflict { keys } => (
                            t_string!(i18n, check_conflict_title).to_owned(),
                            t_string!(i18n, check_conflict_text)
                                .replace("{n}", &keys.len().to_string()),
                        ),
                        NodeCheck::HardwareChanged { models } => (
                            t_string!(i18n, check_hardware_title).to_owned(),
                            t_string!(i18n, check_hardware_text).replace(
                                "{models}",
                                &models
                                    .iter()
                                    .map(|m| crate::mesh::model(m))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ),
                        ),
                    };
                    view! {
                        <li>
                            <strong>{heading}</strong>
                            <span>{text}</span>
                        </li>
                    }
                })
                .collect_view();
            view! { <ul class="node-checks">{unknown} {note} {items}</ul> }
        })
    }
}

/// How the node is doing, at a glance: when it was last heard, how it is
/// reached, what it runs on and how busy it hears the channel. Only what is
/// known shows.
#[component]
fn Tiles(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = move || live.nodes.with(|nodes| nodes.get(&num).cloned());
    let local = move || {
        live.status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
            == Some(num)
    };
    move || {
        let node = node()?;
        let local = local();
        // Of the radio's own, when it was heard and how are no news.
        let heard = node.last_heard.filter(|_| !local).map(|at| {
            view! {
                <Tile label=t!(i18n, nodes_heard).into_any() value=ago(live.now.get(), at) />
            }
        });
        // Heard through others, the signal is the last relay's: only said
        // when it is the node's own.
        let reach = reach_words(i18n, &node, local)
            .filter(|_| !local)
            .map(|words| {
                let signal = node
                    .snr
                    .filter(|_| !local && !node.via_mqtt && node.hops_away == Some(0))
                    .map(|snr| match node.rssi {
                        Some(rssi) => format!("SNR {snr:.1} dB · RSSI {rssi} dBm"),
                        None => format!("SNR {snr:.1} dB"),
                    });
                view! { <Tile label=t!(i18n, nodes_reach).into_any() value=words sub=signal /> }
            });
        let voltage = node.voltage.map(|v| format!("{v:.2} V"));
        let power = match node.battery_level {
            Some(level) if level > 100 => Some(view! {
                <Tile
                    label=t!(i18n, nodes_battery).into_any()
                    value=t_string!(i18n, power_external).to_owned()
                    sub=voltage
                />
            }),
            Some(level) => Some(view! {
                <Tile
                    label=t!(i18n, nodes_battery).into_any()
                    value=format!("{level} %")
                    sub=voltage
                    bar=Some(f64::from(level))
                    low={level < 20}
                />
            }),
            None => voltage.map(|voltage| {
                view! { <Tile label=t!(i18n, nodes_battery).into_any() value=voltage /> }
            }),
        };
        let busy = node.channel_utilization.map(|busy| {
            view! {
                <Tile
                    label=t!(i18n, node_channel_use).into_any()
                    value=format!("{busy:.1} %")
                    bar=Some(f64::from(busy))
                    low={busy > 25.0}
                />
            }
        });
        // The radio's own: how many of the nodes it knows are online, as its
        // stats last said.
        let online = local
            .then(|| {
                let latest = live.latest.get()?;
                let stats = object(latest.local_stats.as_ref(), Some("local_stats"))?;
                let online = number(&stats, "num_online_nodes").unwrap_or_default();
                let total = number(&stats, "num_total_nodes").unwrap_or_default();
                Some(view! {
                    <Tile
                        label=t!(i18n, stats_online).into_any()
                        value=format!("{online:.0}")
                        sub=Some(t_string!(i18n, own_online_of).replace("{total}", &format!("{total:.0}")))
                    />
                })
            })
            .flatten();
        Some(view! { <div class="node-figures">{heard} {reach} {online} {power} {busy}</div> })
    }
}

/// One of the node's figures: what it is, its value, a word more if any, and
/// a bar for a percentage, marked when it is not good.
#[component]
fn Tile(
    label: AnyView,
    value: String,
    #[prop(default = None)] sub: Option<String>,
    #[prop(default = None)] bar: Option<f64>,
    #[prop(optional)] low: bool,
) -> impl IntoView {
    view! {
        <div class="figure" class:low=low>
            <span class="label">{label}</span>
            <span class="value">{value}</span>
            {bar.map(|bar| {
                let width = format!("width: {:.0}%", bar.clamp(0.0, 100.0));
                view! { <span class="bar"><i style=width></i></span> }
            })}
            {sub.map(|sub| view! { <span class="sub">{sub}</span> })}
        </div>
    }
}

/// A part of the page: its title, when what it shows came, and what asks
/// for it anew.
#[component]
fn Section(
    title: AnyView,
    #[prop(default = None)] at: Option<i64>,
    #[prop(default = None)] asked: Option<Asked>,
    #[prop(optional)] class: &'static str,
    children: Children,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let when = at.map(|at| {
        let at = u32::try_from(at).unwrap_or_default();
        move || t_string!(i18n, node_ago).replace("{ago}", &ago(live.now.get(), at))
    });
    let quiet = match asked {
        Some(Asked::Request(kind)) => Some(view! { <Quiet kind /> }),
        _ => None,
    };
    view! {
        <section class=format!("node-section {class}")>
            <header>
                <h2>{title}</h2>
                <span class="when">
                    {when.map(|when| view! { <span>{when}</span> })}
                    {quiet}
                </span>
                {asked.map(|asked| view! { <AskButton asked /> })}
            </header>
            {children()}
        </section>
    }
}

/// Asks the node for something, on the primary channel; beside it, when
/// the channel matters, a menu to send it on another. Off while the radio
/// keeps from sending the like, which it fills up the while; or while the
/// last one waits for its answer.
#[component]
fn AskButton(asked: Asked, #[prop(optional)] place: &'static str) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let OpenMenu(menu) = expect_context::<OpenMenu>();
    // The button pressed last, on any node's page, kept as pages come and go:
    // the one that says the radio keeps from sending the like, and fills up
    // the while. Here, it is this one if it was pressed on this node's page;
    // or, pressed on another's, if it sends the same, in its section.
    let me = ButtonId { asked, place };
    let pressed = live.pressed;
    let is_pressed = move || match pressed.get() {
        Some((node, id)) if node == asking.num => id == me,
        Some((_, id)) => id.asked == asked && place.is_empty(),
        None => false,
    };
    let pressed_here = move || {
        pressed
            .get()
            .filter(|(node, _)| *node == asking.num)
            .map(|(_, id)| id)
    };
    let pending = move || match asked {
        Asked::Request(kind) => live.requests.with(|requests| {
            requests.iter().any(|r| {
                r.node == asking.num && r.kind == kind && r.status == RequestStatus::Pending
            })
        }),
        Asked::Trace => live.traceroutes.with(|traces| {
            traces
                .iter()
                .any(|t| t.target == asking.num && t.status == TracerouteStatus::Pending)
        }),
    };
    // A MeshCore radio traces at no pace.
    let wait = move || {
        if asked == Asked::Trace && live.selected_kind() == Some(RadioKind::Meshcore) {
            return 0;
        }
        asked
            .pace()
            .map_or(0, |pace| live.wait(pace, asking.clock.get()))
    };
    // Not to be sent without its key: the radio refuses to.
    let keyless = move || asked.private() && needs_key(live, asking.num);
    // Logging in to the node first, as asked for something: nothing else
    // goes meanwhile, as it would wait for that login too.
    let logging_in = move || live.logging_in_to(asking.num);
    let off = move || !connected(live) || pending() || wait() > 0 || keyless() || logging_in();
    let send = move |channel: u32| {
        pressed.set(Some((asking.num, me)));
        match asked {
            Asked::Request(kind) => live.ask(asking.num, kind, channel),
            Asked::Trace => live.trace(asking.num, channel),
        }
    };
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect::<Vec<_>>()
        })
    };
    // What goes to it alone goes encrypted to its key, whatever the
    // channel, unless the radio's owner is licensed.
    let by_channel = move || !asked.private() || licensed(live);
    // MeshCore's go to the node, on no channel.
    let has_menu = move || {
        by_channel() && channels().len() > 1 && live.selected_kind() != Some(RadioKind::Meshcore)
    };
    let open = move || menu.get() == Some(me);
    // Asking, said by the button pressed; by every one that sends it when
    // it was asked from elsewhere.
    let asking_here = move || {
        pending() && pressed_here().is_none_or(|pressed| pressed == me || pressed.asked != asked)
    };
    let label = move || {
        if logging_in() && is_pressed() {
            t_string!(i18n, room_logging_in).to_owned()
        } else if asking_here() {
            // A trace is traced; anything else, asked for.
            if asked == Asked::Trace {
                t_string!(i18n, node_tracing).to_owned()
            } else {
                t_string!(i18n, node_asking).to_owned()
            }
        } else if is_pressed() && wait() > 0 {
            // Answered already, but the radio still keeps from sending the
            // like: the button pressed says so while it fills up.
            t_string!(i18n, node_waiting).to_owned()
        } else if asked == Asked::Trace {
            t_string!(i18n, node_trace_button).to_owned()
        } else {
            t_string!(i18n, node_ask_button).to_owned()
        }
    };
    // How long the radio keeps from sending it: said on hovering, shown as
    // the button filling up.
    let paced = move || {
        let wait = wait();
        let pace = asked.pace().filter(|_| wait > 0)?;
        let text = match pace {
            Pace::Position => t_string!(i18n, node_pace_position),
            Pace::Telemetry => t_string!(i18n, node_pace_telemetry),
            Pace::Traceroute => t_string!(i18n, node_pace_traceroute),
        }
        .replace("{n}", &wait.to_string());
        let interval = u32::try_from(pace.interval()).unwrap_or(1).max(1);
        let filled = 100 - wait.min(interval) * 100 / interval;
        Some((text, filled))
    };
    move || {
        (can_ask(live, asked, asking.num) && !own(live, asking.num)).then(|| {
            view! {
                <span class="ask-split" class:open=open>
                    <button
                        class="ask"
                        class:split=has_menu
                        disabled=off
                        title=move || {
                            if keyless() {
                                Some(t_string!(i18n, node_needs_key).to_owned())
                            } else {
                                paced().map(|(text, _)| text)
                            }
                        }
                        on:click=move |_| send(0)
                    >
                        {label}
                        {move || {
                            paced()
                                .filter(|_| is_pressed())
                                .map(|(_, filled)| {
                                    view! { <i class="fill" style=format!("width: {filled}%")></i> }
                                })
                        }}
                    </button>
                    {move || {
                        has_menu()
                            .then(|| {
                                view! {
                                    <button
                                        class="ask caret"
                                        disabled=off
                                        aria-haspopup="menu"
                                        aria-expanded=move || open().to_string()
                                        title=move || t_string!(i18n, node_channel_choose)
                                        aria-label=move || t_string!(i18n, node_channel_choose)
                                        on:click=move |ev| {
                                            ev.stop_propagation();
                                            menu.update(|m| {
                                                *m = if *m == Some(me) { None } else { Some(me) };
                                            });
                                        }
                                    >
                                        <svg viewBox="0 0 24 24" aria-hidden="true">
                                            <path d="M6 9l6 6l6 -6"></path>
                                        </svg>
                                    </button>
                                }
                            })
                    }}
                    <Show when=open>
                        <div class="ask-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                            <p>{t!(i18n, node_channel_menu)}</p>
                            <For each=channels key=|index| *index let:index>
                                <button
                                    role="menuitem"
                                    on:click=move |_| {
                                        menu.set(None);
                                        send(index);
                                    }
                                >
                                    <span class="slot">{index}</span>
                                    {move || title(live, Peer::Channel { channel: index })}
                                </button>
                            </For>
                        </div>
                    </Show>
                </span>
            }
        })
    }
}

/// That the node answered a kind of request lately, and keeps from
/// answering it again for a while, whoever asks.
#[component]
fn Quiet(kind: RequestKind) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    // A Meshtastic node's firmware answers these once in so long; a
    // MeshCore one, whenever asked.
    let meshcore = untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore);
    let window: u32 = match kind {
        _ if meshcore => 0,
        RequestKind::NodeInfo => 60,
        RequestKind::Position | RequestKind::Neighbors => 180,
        _ => 0,
    };
    move || {
        if window == 0 {
            return None;
        }
        let answered = live.requests.with(|requests| {
            requests
                .iter()
                .filter(|r| {
                    r.node == asking.num && r.kind == kind && r.status == RequestStatus::Answered
                })
                .filter_map(|r| r.answered_at)
                .max()
        })?;
        let answered = u32::try_from(answered).ok()?;
        let now = asking.clock.get();
        let since = now.saturating_sub(answered);
        // Short on the heading's line, so that nothing moves for it; all of
        // it on hovering.
        (since < window).then(|| {
            let left = ago(window, since);
            let whole = t_string!(i18n, node_quiet)
                .replace("{ago}", &ago(now, answered))
                .replace("{left}", &left);
            let short = t_string!(i18n, node_quiet_short).replace("{left}", &left);
            view! {
                <span class="quiet" title=whole>
                    {short}
                </span>
            }
        })
    }
}

/// Nothing of a kind told yet.
fn nothing(i18n: I18nContext<Locale>) -> impl IntoView {
    view! { <p class="nothing">{t!(i18n, node_never)}</p> }
}

/// How meshctl reaches the radio: its link and since when, where it is, and
/// what kind it is.
#[component]
fn Link() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let body = move || {
        let radio = live.selected_radio()?;
        let link = live
            .status
            .with(|s| s.as_ref().map(|s| s.link))
            .unwrap_or(radio.link);
        // Since when it is so, while meshctl runs; when it was last up,
        // otherwise, for a radio that is not.
        let since = live.status.with(|s| s.as_ref()?.since);
        let when = match (link, since, radio.last_connected) {
            (_, Some(since), _) => Some(t_string!(i18n, own_since).replace(
                "{ago}",
                &ago(live.now.get(), u32::try_from(since).unwrap_or_default()),
            )),
            (LinkState::Ready, None, _) => None,
            (_, None, Some(last)) => Some(t_string!(i18n, own_last_connected).replace(
                "{ago}",
                &ago(live.now.get(), u32::try_from(last).unwrap_or_default()),
            )),
            _ => None,
        };
        let kind = match radio.kind {
            RadioKind::Meshtastic => "Meshtastic",
            RadioKind::Meshcore => "MeshCore",
        };
        Some(view! {
            <dl class="facts">
                {fact(
                    t!(i18n, own_link),
                    view! {
                        <span class=move || format!("badge {}", live.link_class(link))>
                            <crate::app::LinkLabel link=Signal::stored(link) />
                        </span>
                        {when.map(|when| view! { <span class="dim since">{when}</span> })}
                    },
                )}
                {fact(t!(i18n, own_kind), kind)}
                {wide_fact(t!(i18n, own_address), view! { <span class="mono">{radio.address}</span> })}
            </dl>
        })
    };
    view! {
        <Section title=t!(i18n, own_connection).into_any()>
            {body}
        </Section>
    }
}

/// The radio itself, as it told meshctl on connecting: its firmware, its
/// hardware, what it does in the mesh and where, and its key to share.
#[component]
fn Radio() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let body = move || {
        let local = live.status.with(|s| s.as_ref()?.local.clone())?;
        let locale = i18n.get_locale();
        let key = live
            .nodes
            .with(|nodes| nodes.get(&asking.num).and_then(|n| n.public_key.clone()));
        let key = key.map_or_else(
            || view! { <span class="dim">{t!(i18n, node_key_unknown)}</span> }.into_any(),
            |key| {
                let copied = key.clone();
                view! {
                    <span class="copyable">
                        <span class="mono public-key" title=copied.clone()>{key}</span>
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, node_key_copy)
                            aria-label=move || t_string!(i18n, node_key_copy)
                            inner_html=COPY
                            on:click=move |_| crate::clipboard::copy(&copied)
                        ></button>
                    </span>
                }
                .into_any()
            },
        );
        // A MeshCore radio says its hardware, frequency and modulation in
        // words already, not as the protobufs name Meshtastic's.
        let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
        let preset = local.modem_preset.as_deref().map_or_else(
            || t_string!(i18n, own_custom_lora).to_owned(),
            |p| {
                if meshcore {
                    p.to_owned()
                } else {
                    crate::names::preset(locale, p)
                }
            },
        );
        let hardware = |hw: &str| {
            if meshcore {
                hw.to_owned()
            } else {
                crate::mesh::model(hw)
            }
        };
        let region = |region: &str| {
            if meshcore {
                region.to_owned()
            } else {
                crate::names::region(locale, region)
            }
        };
        Some(view! {
            <dl class="facts">
                {live.nodes.with(|nodes| nodes.get(&local.num).and_then(|own| status_fact(live, own)))}
                {fact(t!(i18n, nodes_id), view! { <span class="mono">{local.id.clone()}</span> })}
                {local.firmware_version.clone().map(|v| fact(t!(i18n, own_firmware), view! { <span class="mono">{v}</span> }))}
                {local.hw_model.as_deref().map(|hw| fact(t!(i18n, nodes_hardware), hardware(hw)))}
                {local.role.as_deref().map(|role| fact(t!(i18n, node_role), crate::names::role(locale, role)))}
                {local.region.as_deref().map(|r| fact(t!(i18n, own_region), region(r)))}
                {fact(t!(i18n, own_preset), preset)}
                {wide_fact(t!(i18n, node_public_key), key)}
                {licensed(live).then(|| fact(t!(i18n, node_licensed), t!(i18n, node_yes)))}
            </dl>
        })
    };
    view! {
        <Section title=t!(i18n, own_radio).into_any()>
            {body}
        </Section>
    }
}

/// Its settings, each tab a click away.
#[component]
fn Shortcuts() -> impl IntoView {
    use crate::settings::Tab;
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // A MeshCore radio's tabs are its own: its kind, read once, as the
    // own page is drawn again for another radio.
    if untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore) {
        use crate::meshcore_settings::{MeshcoreTab, TABS};
        let button = move |tab: MeshcoreTab| {
            let label = move || match tab {
                MeshcoreTab::User => t_string!(i18n, settings_tab_user),
                MeshcoreTab::Radio => t_string!(i18n, mc_tab_radio),
                MeshcoreTab::Channels => t_string!(i18n, settings_tab_channels),
                MeshcoreTab::Advanced => t_string!(i18n, mc_tab_advanced),
            };
            view! {
                <button
                    type="button"
                    on:click=move |_| {
                        live.meshcore_tab.set(tab);
                        live.go(crate::app::Page::RadioSettings);
                    }
                >
                    {label}
                </button>
            }
        };
        return view! {
            <Section title=t!(i18n, own_settings).into_any() class="own-shortcuts">
                <div class="shortcuts pairs">{TABS.into_iter().map(button).collect_view()}</div>
            </Section>
        }
        .into_any();
    }
    let open = move |tab: Tab| {
        live.settings_tab.set(tab);
        live.go(crate::app::Page::RadioSettings);
    };
    let button = move |tab: Tab| {
        let label = move || match tab {
            Tab::User => t_string!(i18n, settings_tab_user),
            Tab::Lora => t_string!(i18n, settings_tab_lora),
            Tab::Channels => t_string!(i18n, settings_tab_channels),
            Tab::Device => t_string!(i18n, settings_tab_device),
            Tab::Modules => t_string!(i18n, settings_tab_modules),
        };
        view! { <button type="button" on:click=move |_| open(tab)>{label}</button> }
    };
    view! {
        <Section title=t!(i18n, own_settings).into_any() class="own-shortcuts">
            <div class="shortcuts">
                {[Tab::User, Tab::Lora, Tab::Channels, Tab::Device, Tab::Modules]
                    .into_iter()
                    .map(button)
                    .collect_view()}
            </div>
        </Section>
    }
    .into_any()
}

/// The path a MeshCore radio knows to a node: straight, by so many hops, or
/// none, when what goes to it floods to find one. Reset, it floods again.
/// Asked for, the way there and back as a flooded request finds them, hop
/// by hop: the one found last, while none is asked anew.
#[component]
fn MeshcorePath() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let busy = RwSignal::new(false);
    let outcome = RwSignal::new(None::<Result<(), Failure>>);
    let node = move || {
        live.nodes.with(|nodes| {
            nodes
                .get(&asking.num)
                .map(|n| (n.hops_away, n.on_radio != Some(false)))
        })
    };
    let reset = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        outcome.set(None);
        spawn_local(async move {
            outcome.set(Some(daemon::reset_path(radio, asking.num).await));
            busy.set(false);
        });
    };
    // The latest asked that ended: the ways found, or none, as it did not
    // answer; and whether it is asked again now.
    let found = move || {
        live.requests.with(|requests| {
            requests
                .iter()
                .filter(|r| {
                    r.node == asking.num
                        && r.kind == RequestKind::Route
                        && r.status != RequestStatus::Pending
                })
                .max_by_key(|r| r.id)
                .map(|r| {
                    let at = r.answered_at.unwrap_or(r.at);
                    match &r.reply {
                        Some(meshctl_api::RequestReply::Route { there, back }) => {
                            (Some((there.clone(), back.clone())), at)
                        }
                        _ => (None, at),
                    }
                })
        })
    };
    let searching = move || {
        live.requests.with(|requests| {
            requests.iter().any(|r| {
                r.node == asking.num
                    && r.kind == RequestKind::Route
                    && r.status == RequestStatus::Pending
            })
        })
    };
    let ways = move || {
        if searching() {
            return None;
        }
        let (ways, _) = found()?;
        let line = |label: AnyView, hops: Vec<meshctl_api::RouteHop>| {
            let way = if hops.is_empty() {
                view! { <span class="way">{t!(i18n, route_straight)}</span> }.into_any()
            } else {
                crate::chat::hop_chain(live, i18n, &hops, true).into_any()
            };
            view! {
                <li>
                    <span class="dim">{label}</span>
                    <span class="hops">{way}</span>
                </li>
            }
        };
        Some(match ways {
            Some((there, back)) => {
                let guessed = crate::chat::any_guessed(&there) || crate::chat::any_guessed(&back);
                view! {
                    <div class="routes found-route">
                        <ol>
                            {line(t!(i18n, mc_route_there).into_any(), there)}
                            {line(t!(i18n, mc_route_back).into_any(), back)}
                        </ol>
                        {guessed.then(|| view! { <p class="hint dim guessed">{t!(i18n, route_guess_hint)}</p> })}
                    </div>
                }
                .into_any()
            }
            None => view! { <p class="hint dim">{t!(i18n, mc_route_lost)}</p> }.into_any(),
        })
    };
    let body = move || {
        let (hops, contact) = node().unwrap_or((None, false));
        let path = match hops {
            Some(0) => t_string!(i18n, mc_path_direct).to_owned(),
            Some(1) => t_string!(i18n, mc_path_hop).to_owned(),
            Some(n) => t_string!(i18n, chat_hops).replace("{n}", &n.to_string()),
            None => t_string!(i18n, mc_path_none).to_owned(),
        };
        view! {
            <dl class="facts compact">{wide_fact(t!(i18n, mc_path_known), path)}</dl>
            <p class="hint dim">
                {if contact {
                    t!(i18n, mc_path_hint).into_any()
                } else {
                    t!(i18n, mc_path_not_contact).into_any()
                }}
            </p>
            <div class="path-actions">
                <button
                    type="button"
                    disabled=move || busy.get() || !contact || hops.is_none() || !connected(live)
                    on:click=reset
                >
                    {t!(i18n, mc_path_reset)}
                </button>
                {move || outcome.get().map(|done| match done {
                    Ok(()) => view! { <span class="dim">{t!(i18n, mc_path_forgotten)}</span> }.into_any(),
                    Err(_) => view! { <span class="problem">{t!(i18n, error_failed)}</span> }.into_any(),
                })}
            </div>
            {contact.then(|| view! { <p class="hint dim">{t!(i18n, mc_route_hint)}</p> })}
            {ways}
        }
    };
    // Asked for of a contact only, as the radio asks only those.
    move || {
        let contact = node().is_some_and(|(_, contact)| contact);
        view! {
            <Section
                title=t!(i18n, mc_path).into_any()
                class="mc-path"
                at=found().map(|(_, at)| at)
                asked=contact.then_some(Asked::Request(RequestKind::Route))
            >
                {body}
            </Section>
        }
    }
}

/// The radio says who it is, for others to add it: to those it reaches
/// straight, or through the repeaters to the whole mesh. It goes on the air.
#[component]
fn Advert() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let busy = RwSignal::new(false);
    let outcome = RwSignal::new(None::<Result<bool, Failure>>);
    let send = move |flood: bool| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        outcome.set(None);
        spawn_local(async move {
            let done = daemon::advert(radio, flood).await;
            outcome.set(Some(done.map(|()| flood)));
            busy.set(false);
        });
    };
    view! {
        <Section title=t!(i18n, mc_advert).into_any() class="mc-advert">
            <p class="hint dim">{t!(i18n, mc_advert_hint)}</p>
            <div class="path-actions advert-actions">
                <button
                    type="button"
                    disabled=move || busy.get() || !connected(live)
                    on:click=move |_| send(false)
                >
                    {t!(i18n, mc_advert_nearby)}
                </button>
                <button
                    type="button"
                    disabled=move || busy.get() || !connected(live)
                    on:click=move |_| send(true)
                >
                    {t!(i18n, mc_advert_flood)}
                </button>
                {move || outcome.get().map(|done| match done {
                    Ok(false) => view! { <span class="dim">{t!(i18n, mc_advert_sent_nearby)}</span> }.into_any(),
                    Ok(true) => view! { <span class="dim">{t!(i18n, mc_advert_sent_flood)}</span> }.into_any(),
                    Err(_) => view! { <span class="problem">{t!(i18n, error_failed)}</span> }.into_any(),
                })}
            </div>
        </Section>
    }
}

/// A MeshCore radio's figures: its battery and storage, how long it has
/// been on, how it hears the air and what it has sent and received. Asked
/// of it on showing, and every half minute while shown.
#[component]
fn MeshcoreStatus() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let status = RwSignal::new(None::<meshctl_api::MeshcoreStatus>);
    let read = move || {
        if let Some(radio) = live.selected.get_untracked() {
            spawn_local(async move {
                if let Ok(read) = daemon::meshcore_status(radio).await {
                    status.set(Some(read));
                }
            });
        }
    };
    read();
    if let Ok(every) = set_interval_with_handle(read, Duration::from_secs(30)) {
        on_cleanup(move || every.clear());
    }
    let body = move || {
        let Some(s) = status.get() else {
            return nothing(i18n).into_any();
        };
        let number = |n: u32| n.to_string();
        // A share of the time it has been on.
        #[allow(clippy::cast_precision_loss)]
        let share = |secs: Option<u32>| match (secs, s.uptime) {
            (Some(secs), Some(up)) if up > 0 => {
                Some(format!("{:.1} %", secs as f64 * 100.0 / up as f64))
            }
            _ => None,
        };
        view! {
            <dl class="facts compact">
                {s.battery_mv.map(|mv| fact(t!(i18n, nodes_battery), format!("{:.2} V", f64::from(mv) / 1000.0)))}
                {s.uptime.map(|up| fact(t!(i18n, node_uptime), duration(u64::from(up))))}
                {match (s.storage_used_kb, s.storage_total_kb) {
                    (Some(used), Some(total)) if total > 0 => Some(fact(
                        t!(i18n, mc_storage),
                        t_string!(i18n, mc_storage_of)
                            .replace("{used}", &used.to_string())
                            .replace("{total}", &total.to_string()),
                    )),
                    _ => None,
                }}
                {s.noise_floor.map(|n| fact(t!(i18n, stats_noise), format!("{n} dBm")))}
                {match (s.last_rssi, s.last_snr) {
                    (Some(rssi), Some(snr)) => Some(fact(t!(i18n, mc_last_heard), format!("{rssi} dBm · {snr:.1} dB"))),
                    _ => None,
                }}
                {share(s.tx_air_secs).map(|v| fact(t!(i18n, node_air_tx), v))}
                {share(s.rx_air_secs).map(|v| fact(t!(i18n, mc_air_rx), v))}
                {s.packets_sent.map(|n| fact(t!(i18n, stats_packets_tx), number(n)))}
                {s.packets_received.map(|n| fact(t!(i18n, stats_packets_rx), number(n)))}
                {match (s.sent_flood, s.sent_direct) {
                    (Some(flood), Some(direct)) => Some(fact(
                        t!(i18n, mc_sent_how),
                        t_string!(i18n, mc_flood_direct)
                            .replace("{flood}", &flood.to_string())
                            .replace("{direct}", &direct.to_string()),
                    )),
                    _ => None,
                }}
                {match (s.received_flood, s.received_direct) {
                    (Some(flood), Some(direct)) => Some(fact(
                        t!(i18n, mc_received_how),
                        t_string!(i18n, mc_flood_direct)
                            .replace("{flood}", &flood.to_string())
                            .replace("{direct}", &direct.to_string()),
                    )),
                    _ => None,
                }}
                {s.receive_errors.map(|n| fact(t!(i18n, stats_rx_bad), number(n)))}
                {s.queue.map(|n| fact(t!(i18n, mc_queue), n.to_string()))}
            </dl>
        }
        .into_any()
    };
    view! {
        <Section title=t!(i18n, mc_status).into_any()>{body}</Section>
    }
}

/// The radio's own node to share: a QR its kind's app adds as a contact,
/// and the same link as text, to copy. Read again as the radio says who it
/// is, or changes.
#[component]
fn Share() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let url = RwSignal::new(None::<String>);
    // Who the radio is: its id, its names, its key; the link follows them.
    let who = Memo::new(move |_| {
        let local = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num))?;
        live.nodes.with(|nodes| {
            nodes.get(&local).map(|n| {
                (
                    local,
                    n.long_name.clone(),
                    n.short_name.clone(),
                    n.public_key.clone(),
                )
            })
        })
    });
    Effect::new(move |_| {
        let (Some(radio), Some(_)) = (live.selected.get(), who.get()) else {
            url.set(None);
            return;
        };
        spawn_local(async move {
            url.set(daemon::contact(radio).await.ok().map(|c| c.url));
        });
    });
    move || {
        let link = url.get()?;
        Some(view! {
            <Section title=t!(i18n, own_contact).into_any() class="contact-share">
                {contact_card(link, true)}
            </Section>
        })
    }
}

/// A node's contact to share: its QR, dark on light whatever the theme as
/// scanners read best, beside how to use it (`own`, the radio's) and the
/// link to copy; the QR over the words while they do not fit beside it.
fn contact_card(link: String, own: bool) -> Option<AnyView> {
    let i18n = use_i18n();
    // Whose apps open it, as the link itself says.
    let meshcore = link.starts_with("meshcore://");
    link_card(link, move || match (own, meshcore) {
        (true, false) => t!(i18n, own_contact_hint_meshtastic).into_any(),
        (true, true) => t!(i18n, own_contact_hint_meshcore).into_any(),
        (false, false) => t!(i18n, node_contact_hint_meshtastic).into_any(),
        (false, true) => t!(i18n, node_contact_hint_meshcore).into_any(),
    })
}

/// A link to share, in a `.contact-share`: its QR beside how to use it and
/// the link to copy.
pub(crate) fn link_card(
    link: String,
    hint: impl Fn() -> AnyView + Send + Sync + 'static,
) -> Option<AnyView> {
    let i18n = use_i18n();
    let svg = qrcode::QrCode::new(link.as_bytes())
        .ok()?
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#111111"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    let copied = link.clone();
    Some(
        view! {
            <div class="contact">
                <div class="qr" role="img" aria-label=move || t_string!(i18n, own_contact_qr) inner_html=svg></div>
                <div class="contact-words">
                    <p class="hint">{hint}</p>
                    <span class="copyable">
                        <span class="mono contact-link" title=link.clone()>{link.clone()}</span>
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, own_contact_copy)
                            aria-label=move || t_string!(i18n, own_contact_copy)
                            inner_html=COPY
                            on:click=move |_| crate::clipboard::copy(&copied)
                        ></button>
                    </span>
                </div>
            </div>
        }
        .into_any(),
    )
}

/// A node's contact to share, over the page: asked of the daemon as it
/// opens. Escape, its ✕ or a click off it close it.
#[component]
fn ContactModal(num: u32, open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let url = RwSignal::new(None::<Option<String>>);
    if let Some(radio) = live.selected.get_untracked() {
        spawn_local(async move {
            url.set(Some(
                daemon::node_contact(radio, num).await.ok().map(|c| c.url),
            ));
        });
    }
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            open.set(false);
        }
    });
    on_cleanup(move || escape.remove());
    let name = move || crate::chat::title(live, Peer::Node { node: num });
    let body = move || match url.get() {
        None => None,
        Some(Some(link)) => contact_card(link, false),
        Some(None) => Some(view! { <p class="dim">{t!(i18n, node_contact_none)}</p> }.into_any()),
    };
    view! {
        <div class="modal-back" on:click=move |_| open.set(false)>
            <div
                class="card contact-modal contact-share"
                role="dialog"
                aria-modal="true"
                on:click=|ev| ev.stop_propagation()
            >
                <header>
                    <h2>{move || t_string!(i18n, node_contact_title).replace("{name}", &name())}</h2>
                    <button
                        class="icon-button"
                        title=move || t_string!(i18n, drawer_close)
                        aria-label=move || t_string!(i18n, drawer_close)
                        inner_html=CLOSE
                        on:click=move |_| open.set(false)
                    ></button>
                </header>
                <div class="modal-body">{body}</div>
            </div>
        </div>
    }
}

/// Where the node is, and how well that is known.
#[component]
fn PositionSection(at: Option<i64>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let body = move || {
        let Some(position) = live
            .nodes
            .with(|nodes| nodes.get(&asking.num).and_then(|n| n.position))
        else {
            return nothing(i18n).into_any();
        };
        let place = format!("{:.5}, {:.5}", position.latitude, position.longitude);
        let copied = place.clone();
        let source = position.source.map(|source| match source {
            PositionSource::Manual => t_string!(i18n, position_manual).to_owned(),
            PositionSource::Internal => t_string!(i18n, position_internal).to_owned(),
            PositionSource::External => t_string!(i18n, position_external).to_owned(),
        });
        view! {
            <dl class="facts">
                {fact(
                    t!(i18n, node_coordinates),
                    view! {
                        <span class="copyable">
                            <span class="mono">{place}</span>
                            <button
                                class="icon-button"
                                title=move || t_string!(i18n, node_coordinates_copy)
                                aria-label=move || t_string!(i18n, node_coordinates_copy)
                                inner_html=COPY
                                on:click=move |_| crate::clipboard::copy(&copied)
                            ></button>
                        </span>
                    },
                )}
                {position.altitude.map(|a| fact(t!(i18n, node_altitude), format!("{a} m")))}
                {position.radius.map(|r| fact(t!(i18n, node_precision), radius(r)))}
                {source.map(|s| fact(t!(i18n, node_source), s))}
            </dl>
        }
        .into_any()
    };
    // The small map apart from the facts, built once: what it shows changes
    // with the node, and the map stays.
    let placed = Memo::new(move |_| {
        live.nodes
            .with(|nodes| nodes.get(&asking.num).is_some_and(|n| n.position.is_some()))
    });
    view! {
        <Section title=t!(i18n, node_position).into_any() at asked=Some(Asked::Request(RequestKind::Position))>
            <Show when=move || placed.get()>
                <crate::map::MiniMap num=asking.num />
            </Show>
            {body}
        </Section>
    }
}

/// Who the node is: its id, its public key, and whether its owner is
/// licensed. Its names, role and hardware are in the heading.
#[component]
fn Identity(at: Option<i64>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let sharing = RwSignal::new(false);
    let body = move || {
        let node = live.nodes.with(|nodes| nodes.get(&asking.num).cloned())?;
        let key = match node.public_key.clone() {
            Some(key) => {
                let copied = key.clone();
                view! {
                    <span class="copyable">
                        <span class="mono public-key" title=copied.clone()>{key}</span>
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, node_key_copy)
                            aria-label=move || t_string!(i18n, node_key_copy)
                            inner_html=COPY
                            on:click=move |_| crate::clipboard::copy(&copied)
                        ></button>
                        // Its contact, to add it elsewhere: there is one
                        // only with its key.
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, node_contact_show)
                            aria-label=move || t_string!(i18n, node_contact_show)
                            inner_html=QR
                            on:click=move |_| sharing.set(true)
                        ></button>
                    </span>
                }
                .into_any()
            }
            None => view! { <span class="dim">{t!(i18n, node_key_unknown)}</span> }.into_any(),
        };
        // How what is for it alone reaches it: telemetry and neighbours
        // asked, and messages to it.
        let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
        let direct = if meshcore {
            t!(i18n, node_direct_meshcore).into_any()
        } else if licensed(live) {
            t!(i18n, node_direct_licensed).into_any()
        } else if node.public_key.is_some() {
            t!(i18n, node_direct_pki).into_any()
        } else {
            t!(i18n, node_direct_none).into_any()
        };
        Some(view! {
            <dl class="facts">
                {status_fact(live, &node)}
                {fact(t!(i18n, nodes_id), view! { <span class="mono">{node.id.clone()}</span> })}
                {wide_fact(t!(i18n, node_public_key), key)}
                {wide_fact(t!(i18n, node_direct), direct)}
                {node.licensed.then(|| fact(t!(i18n, node_licensed), t!(i18n, node_yes)))}
            </dl>
        })
    };
    view! {
        <Section title=t!(i18n, node_identity).into_any() at asked=Some(Asked::Request(RequestKind::NodeInfo))>
            {body}
        </Section>
        <Show when=move || sharing.get()>
            <ContactModal num=asking.num open=sharing />
        </Show>
    }
}

/// What its device telemetry says: its battery, how much it sends, how long
/// it has been on.
#[component]
fn Device(packet: Option<Packet>) -> impl IntoView {
    let i18n = use_i18n();
    let at = packet.as_ref().map(|p| p.at);
    // Its words follow the language: read where that is tracked.
    let body = move || match object(packet.as_ref(), Some("device_metrics")) {
        Some(metrics) => {
            let battery = number(&metrics, "battery_level").map(|level| {
                if level > 100.0 {
                    t_string!(i18n, power_external).to_owned()
                } else {
                    format!("{level:.0} %")
                }
            });
            view! {
                <dl class="facts compact">
                    {battery.map(|b| fact(t!(i18n, node_battery), b))}
                    {number(&metrics, "voltage").map(|v| fact(t!(i18n, node_voltage), format!("{v:.2} V")))}
                    {number(&metrics, "channel_utilization")
                        .map(|c| fact(t!(i18n, node_channel_use), format!("{c:.1} %")))}
                    {number(&metrics, "air_util_tx").map(|a| fact(t!(i18n, node_air_tx), format!("{a:.1} %")))}
                    {number(&metrics, "uptime_seconds")
                        .map(|u| fact(t!(i18n, node_uptime), duration(u as u64)))}
                </dl>
            }
            .into_any()
        }
        None => nothing(i18n).into_any(),
    };
    view! {
        <Section title=t!(i18n, node_device).into_any() at asked=Some(Asked::Request(RequestKind::DeviceTelemetry))>
            {body}
        </Section>
    }
}

/// What a value of an environment sensor is, in the interface's words, its
/// unit and how many decimals it is read with.
pub(crate) fn environment_field(
    i18n: I18nContext<Locale>,
    key: &str,
) -> Option<(String, &'static str, usize)> {
    let (label, unit, decimals) = match key {
        "temperature" => (t_string!(i18n, env_temperature), " °C", 1),
        "relative_humidity" => (t_string!(i18n, env_humidity), " %", 0),
        "barometric_pressure" => (t_string!(i18n, env_pressure), " hPa", 1),
        "gas_resistance" => (t_string!(i18n, env_gas), " MΩ", 2),
        "iaq" => (t_string!(i18n, env_iaq), "", 0),
        "lux" => (t_string!(i18n, env_lux), " lx", 0),
        "uv_lux" => (t_string!(i18n, env_uv_lux), " lx", 1),
        "wind_speed" => (t_string!(i18n, env_wind_speed), " m/s", 1),
        "wind_gust" => (t_string!(i18n, env_wind_gust), " m/s", 1),
        "wind_direction" => (t_string!(i18n, env_wind_direction), "°", 0),
        "rainfall_1h" => (t_string!(i18n, env_rain_1h), " mm", 1),
        "rainfall_24h" => (t_string!(i18n, env_rain_24h), " mm", 1),
        "distance" => (t_string!(i18n, env_distance), " mm", 0),
        "weight" => (t_string!(i18n, env_weight), " kg", 2),
        "radiation" => (t_string!(i18n, env_radiation), " µR/h", 2),
        "soil_moisture" => (t_string!(i18n, env_soil_moisture), " %", 0),
        "soil_temperature" => (t_string!(i18n, env_soil_temperature), " °C", 1),
        "voltage" => (t_string!(i18n, env_voltage), " V", 2),
        "current" => (t_string!(i18n, env_current), " mA", 0),
        _ => return None,
    };
    Some((label.to_owned(), unit, decimals))
}

/// The order environment values are shown in.
pub(crate) const ENVIRONMENT_FIELDS: [&str; 19] = [
    "temperature",
    "relative_humidity",
    "barometric_pressure",
    "gas_resistance",
    "iaq",
    "lux",
    "uv_lux",
    "wind_speed",
    "wind_gust",
    "wind_direction",
    "rainfall_1h",
    "rainfall_24h",
    "distance",
    "weight",
    "radiation",
    "soil_moisture",
    "soil_temperature",
    "voltage",
    "current",
];

/// What its environment sensors read.
fn environment_facts(i18n: I18nContext<Locale>, packet: Option<&Packet>) -> Option<AnyView> {
    let metrics = object(packet, Some("environment_metrics"))?;
    let facts = ENVIRONMENT_FIELDS
        .iter()
        .filter_map(|key| {
            let value = number(&metrics, key)?;
            let (label, unit, decimals) = environment_field(i18n, key)?;
            Some(fact(label, format!("{value:.decimals$}{unit}")))
        })
        .collect_view();
    Some(view! { <dl class="facts compact">{facts}</dl> }.into_any())
}

/// What its power sensors read, channel by channel.
fn power_facts(i18n: I18nContext<Locale>, packet: Option<&Packet>) -> Option<AnyView> {
    let metrics = object(packet, Some("power_metrics"))?;
    let facts = (1..=8)
        .filter_map(|channel| {
            let voltage = number(&metrics, &format!("ch{channel}_voltage"));
            let current = number(&metrics, &format!("ch{channel}_current"));
            let value = match (voltage, current) {
                (Some(v), Some(c)) => format!("{v:.2} V · {c:.0} mA"),
                (Some(v), None) => format!("{v:.2} V"),
                (None, Some(c)) => format!("{c:.0} mA"),
                (None, None) => return None,
            };
            let label = t_string!(i18n, power_channel).replace("{n}", &channel.to_string());
            Some(fact(label, value))
        })
        .collect_view();
    Some(view! { <dl class="facts compact">{facts}</dl> }.into_any())
}

/// How many neighbours show before "see all": three rows of two, so that a
/// node that hears many does not make its column the longest.
const NEIGHBOURS_SHOWN: usize = 6;

/// The nodes it hears directly, each opening its page if the radio knows
/// of it: the best heard first, the first few, all of them while `all`.
fn neighbour_list(
    i18n: I18nContext<Locale>,
    live: Live,
    packet: Option<&Packet>,
    all: RwSignal<bool>,
) -> Option<AnyView> {
    let info = object(packet, None)?;
    let mut neighbours: Vec<(u32, Option<f64>)> = info
        .get("neighbors")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|n| {
                    let num = u32::try_from(n.get("node_id")?.as_u64()?).ok()?;
                    Some((num, n.get("snr").and_then(Value::as_f64)))
                })
                .collect()
        })
        .unwrap_or_default();
    if neighbours.is_empty() {
        return Some(view! { <p class="nothing">{t!(i18n, neighbors_none)}</p> }.into_any());
    }
    neighbours.sort_by(|a, b| {
        b.1.unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.1.unwrap_or(f64::NEG_INFINITY))
    });
    let count = neighbours.len();
    let rows = neighbours
        .into_iter()
        .enumerate()
        .map(|(index, (num, snr))| {
            let known = live.nodes.with_untracked(|nodes| nodes.contains_key(&num));
            let name = move || title(live, Peer::Node { node: num });
            let who = if known {
                view! {
                    <button class="link" on:click=move |_| live.open_node(num)>
                        {name}
                    </button>
                }
                .into_any()
            } else {
                view! { <span class="mono">{name}</span> }.into_any()
            };
            view! {
                <li class:extra=(index >= NEIGHBOURS_SHOWN)>
                    {who}
                    <span class="snr">{snr.map(|s| format!("{s:.1} dB"))}</span>
                </li>
            }
        })
        .collect_view();
    let more = (count > NEIGHBOURS_SHOWN).then(|| {
        let label = move || {
            if all.get() {
                t_string!(i18n, neighbors_fewer).to_owned()
            } else {
                t_string!(i18n, neighbors_all).replace("{n}", &count.to_string())
            }
        };
        view! {
            <button
                class="link neighbours-more"
                aria-expanded=move || all.get().to_string()
                on:click=move |_| all.update(|all| *all = !*all)
            >
                {label}
            </button>
        }
    });
    Some(
        view! {
            <ul class="neighbours" class:all=move || all.get()>{rows}</ul>
            {more}
        }
        .into_any(),
    )
}

/// What the node tells only with the sensors or the module for it: always
/// all three, in the same place, each growing where it is when it tells.
#[component]
fn Sensors(
    latest: NodeLatest,
    /// The radio's own node, whose modules are known: only those it has
    /// on, or that told something, are shown.
    #[prop(optional)]
    own: bool,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let at = |packet: &Option<Packet>| packet.as_ref().map(|p| p.at);
    // Which of them the radio has on, as its settings say: environment,
    // power, neighbours. Read from what the daemon keeps, nothing asked of
    // the radio.
    let enabled = RwSignal::new(None::<[bool; 3]>);
    // MeshCore radios have no such settings yet: nothing to ask. Asked
    // once, as the section is built: not followed.
    if own
        && untrack(|| live.selected_kind()) != Some(meshctl_api::RadioKind::Meshcore)
        && let Some(radio) = live.selected.get_untracked()
    {
        spawn_local(async move {
            if let Ok(sections) = daemon::sections(radio).await {
                enabled.set(Some(modules_on(&sections)));
            }
        });
    }
    // Whether all the neighbours show: kept here, out of what is built
    // anew as the language changes.
    let all_neighbours = RwSignal::new(false);
    // Its words follow the language: built where that is tracked.
    move || {
        // Here, and not as the parts' properties: those are built untracked.
        let environment = environment_facts(i18n, latest.environment.as_ref());
        let power = power_facts(i18n, latest.power.as_ref());
        let neighbours = neighbour_list(i18n, live, latest.neighbors.as_ref(), all_neighbours);
        let latest = latest.clone();
        // Of a node heard, all of them, as it may answer; of the radio's own,
        // those that told something or are on.
        let on = enabled.get();
        let shown = |index: usize, told: bool| !own || told || on.is_some_and(|on| on[index]);
        let parts = [
            shown(0, environment.is_some()),
            shown(1, power.is_some()),
            shown(2, neighbours.is_some()),
        ];
        if !parts.contains(&true) {
            return None;
        }
        Some(view! {
            <Section title=t!(i18n, node_sensors).into_any() class="node-sensors">
                <p class="hint">
                    {if own {
                        t!(i18n, node_sensors_own_hint).into_any()
                    } else {
                        t!(i18n, node_sensors_hint).into_any()
                    }}
                </p>
                {parts[0].then(|| view! {
                    <Part
                        kind=RequestKind::EnvironmentTelemetry
                        title=t!(i18n, node_environment).into_any()
                        at=at(&latest.environment)
                        body=environment
                    />
                })}
                {parts[1].then(|| view! {
                    <Part
                        kind=RequestKind::PowerTelemetry
                        title=t!(i18n, node_power).into_any()
                        at=at(&latest.power)
                        body=power
                    />
                })}
                {parts[2].then(|| view! {
                    <Part
                        kind=RequestKind::Neighbors
                        title=t!(i18n, node_neighbors).into_any()
                        at=at(&latest.neighbors)
                        body=neighbours
                    />
                })}
            </Section>
        })
    }
}

/// Which of the sensors and modules a radio has on, as its settings say:
/// the telemetry module's environment and power measurement, and the
/// neighbour info module.
fn modules_on(sections: &[SettingsSection]) -> [bool; 3] {
    let on = |section: &str, field: &str| {
        sections
            .iter()
            .find(|s| s.id == section)
            .and_then(|s| s.fields.iter().find(|f| f.name == field))
            .and_then(|f| f.value.as_ref())
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    [
        on("module.telemetry", "environment_measurement_enabled"),
        on("module.telemetry", "power_measurement_enabled"),
        on("module.neighbor_info", "enabled"),
    ]
}

/// One of the sensors or modules: its heading as a section's, and what it
/// told, or in one line why nothing yet.
#[component]
fn Part(
    kind: RequestKind,
    title: AnyView,
    at: Option<i64>,
    body: Option<AnyView>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let when = at.map(|at| {
        let at = u32::try_from(at).unwrap_or_default();
        move || t_string!(i18n, node_ago).replace("{ago}", &ago(live.now.get(), at))
    });
    let body = body.unwrap_or_else(|| {
        let status = move || {
            if refused(live, asking.num, kind) {
                match kind {
                    RequestKind::EnvironmentTelemetry => t!(i18n, node_no_environment).into_any(),
                    RequestKind::PowerTelemetry => t!(i18n, node_no_power).into_any(),
                    _ => t!(i18n, node_no_neighbors).into_any(),
                }
            } else {
                t!(i18n, node_never).into_any()
            }
        };
        view! { <p class="nothing">{status}</p> }.into_any()
    });
    view! {
        <div class="part">
            <header>
                <h3>{title}</h3>
                <span class="when">
                    {when.map(|when| view! { <span>{when}</span> })}
                    <Quiet kind />
                </span>
                <AskButton asked=Asked::Request(kind) />
            </header>
            {body}
        </div>
    }
}

/// Whether the latest time the node was asked for a kind, and it answered
/// or gave up, it said it has nothing to tell: kept while it is asked anew,
/// so that the line stays until another takes its place.
fn refused(live: Live, num: u32, kind: RequestKind) -> bool {
    live.requests.with(|requests| {
        requests
            .iter()
            .find(|r| r.node == num && r.kind == kind && r.status != RequestStatus::Pending)
            .is_some_and(|r| {
                r.status == RequestStatus::Failed && r.error.as_deref() == Some("NO_RESPONSE")
            })
    })
}

/// How it sees the mesh.
#[component]
fn Stats(packet: Option<Packet>) -> impl IntoView {
    let i18n = use_i18n();
    let at = packet.as_ref().map(|p| p.at);
    // Its words follow the language: read where that is tracked.
    let body = move || match object(packet.as_ref(), Some("local_stats")) {
        Some(stats) => {
            // What a protobuf leaves at zero its JSON leaves out.
            let count = |key: &str| number(&stats, key).unwrap_or_default();
            let online = t_string!(i18n, stats_online_of)
                .replace("{online}", &format!("{:.0}", count("num_online_nodes")))
                .replace("{total}", &format!("{:.0}", count("num_total_nodes")));
            let counted = |label: String, key: &str| fact(label, format!("{:.0}", count(key)));
            view! {
                <dl class="facts compact">
                    {fact(t!(i18n, stats_online), online)}
                    {counted(t_string!(i18n, stats_packets_tx).to_owned(), "num_packets_tx")}
                    {counted(t_string!(i18n, stats_packets_rx).to_owned(), "num_packets_rx")}
                    {counted(t_string!(i18n, stats_rx_bad).to_owned(), "num_packets_rx_bad")}
                    {counted(t_string!(i18n, stats_dupes).to_owned(), "num_rx_dupe")}
                    {counted(t_string!(i18n, stats_relayed).to_owned(), "num_tx_relay")}
                    {counted(t_string!(i18n, stats_relay_canceled).to_owned(), "num_tx_relay_canceled")}
                    {counted(t_string!(i18n, stats_dropped).to_owned(), "num_tx_dropped")}
                    {number(&stats, "noise_floor")
                        .map(|n| fact(t!(i18n, stats_noise), format!("{n:.0} dBm")))}
                    {fact(t!(i18n, node_channel_use), format!("{:.1} %", count("channel_utilization")))}
                    {fact(t!(i18n, node_air_tx), format!("{:.1} %", count("air_util_tx")))}
                    {number(&stats, "uptime_seconds")
                        .map(|u| fact(t!(i18n, node_uptime), duration(u as u64)))}
                </dl>
            }
            .into_any()
        }
        None => nothing(i18n).into_any(),
    };
    view! {
        <Section title=t!(i18n, node_stats).into_any() at asked=Some(Asked::Request(RequestKind::LocalStats))>
            {body}
        </Section>
    }
}

/// The latest route traced to the node, and the way to all of them.
#[component]
fn Traces(drawer: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || {
        live.traceroutes
            .with(|traces| traces.iter().find(|t| t.target == asking.num).cloned())
    };
    // Earlier ones, the history: only then is there anything to see.
    let earlier = move || {
        live.traceroutes
            .with(|traces| traces.iter().filter(|t| t.target == asking.num).count() > 1)
    };
    view! {
        <Section title=t!(i18n, node_traces).into_any() asked=Some(Asked::Trace) class="node-traces">
            {move || match last() {
                Some(traceroute) => view! {
                    <TraceCard traceroute />
                    <Show when=earlier>
                        <button class="link all" on:click=move |_| drawer.set(true)>
                            {t!(i18n, traces_all)}
                        </button>
                    </Show>
                }
                .into_any(),
                None => view! { <p class="nothing">{t!(i18n, traces_none)}</p> }.into_any(),
            }}
            // A room or a sensor answers only when set to repeat.
            {move || {
                let repeats_if_set = live.nodes.with(|nodes| {
                    nodes
                        .get(&asking.num)
                        .is_some_and(|n| matches!(n.role.as_deref(), Some("ROOM" | "SENSOR")))
                });
                (repeats_if_set && meshcore_traceable(live, asking.num))
                    .then(|| view! { <p class="hint dim">{t!(i18n, traces_repeat_hint)}</p> })
            }}
            // Why not, when a MeshCore one cannot be traced now.
            {move || {
                (live.selected_kind() == Some(RadioKind::Meshcore)
                    && !meshcore_traceable(live, asking.num))
                    .then(|| view! { <p class="hint dim">{t!(i18n, traces_meshcore_hint)}</p> })
            }}
        </Section>
    }
}

/// A drawer over the page's side, for a list too long for the page: its
/// title, what it holds, and closing it, by its button, by a click beside
/// it or by Escape.
#[component]
pub(crate) fn Drawer(
    open: RwSignal<bool>,
    title: Signal<String>,
    /// What can be done with what it holds: at its foot, always in sight.
    #[prop(optional)]
    foot: Option<ViewFn>,
    children: ChildrenFn,
) -> impl IntoView {
    let i18n = use_i18n();
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            open.set(false);
        }
    });
    on_cleanup(move || escape.remove());
    view! {
        <Show when=move || open.get()>
            <div class="drawer-shade" on:click=move |_| open.set(false)></div>
            <aside class="drawer" role="dialog" aria-modal="true">
                <header>
                    <h2>{move || title.get()}</h2>
                    <button
                        class="icon-button"
                        title=move || t_string!(i18n, drawer_close)
                        aria-label=move || t_string!(i18n, drawer_close)
                        on:click=move |_| open.set(false)
                    >
                        <svg viewBox="0 0 24 24" aria-hidden="true">
                            <path d="M18 6L6 18M6 6l12 12"></path>
                        </svg>
                    </button>
                </header>
                <div class="drawer-body">{children()}</div>
                {foot.clone().map(|foot| view! { <footer class="drawer-foot">{foot.run()}</footer> })}
            </aside>
        </Show>
    }
}

/// What a history is, for its button that forgets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum History {
    Requests,
    Traceroutes,
}

/// Forgets a history of the node's, once more said sure: it cannot be
/// undone. What is forgotten is read again after.
#[component]
fn Forget(history: History, reload: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let sure = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let forget = move |_| {
        busy.set(true);
        spawn_local(async move {
            let done = match history {
                History::Requests => live.forget_requests(asking.num).await,
                History::Traceroutes => live.forget_traceroutes(asking.num).await,
            };
            busy.set(false);
            sure.set(false);
            if done.is_ok() {
                reload.run(());
            }
        });
    };
    let question = move || match history {
        History::Requests => t_string!(i18n, forget_requests_sure).to_owned(),
        History::Traceroutes => t_string!(i18n, forget_traces_sure).to_owned(),
    };
    view! {
        <div class="forget">
            <Show
                when=move || sure.get()
                fallback=move || {
                    view! {
                        <button class="link" on:click=move |_| sure.set(true)>
                            {t!(i18n, forget_history)}
                        </button>
                    }
                }
            >
                <span class="question">{question}</span>
                <button class="danger" disabled=move || busy.get() on:click=forget>
                    {t!(i18n, forget_yes)}
                </button>
                <button disabled=move || busy.get() on:click=move |_| sure.set(false)>
                    {t!(i18n, forget_no)}
                </button>
            </Show>
        </div>
    }
}

/// Forgetting the node: by the radio alone, or by meshctl too, all it kept
/// of it. Each once more said sure, as neither can be undone; done once the
/// radio says it did.
#[component]
fn RemoveDrawer() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let title = Signal::derive(move || t_string!(i18n, remove_title).to_owned());
    let problem = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    view! {
        <Drawer open=live.removing title>
            <div class="removal">
                // Out of the radio, once: then all of it, or nothing.
                {move || {
                    let on_radio = live
                        .nodes
                        .with(|nodes| nodes.get(&asking.num).and_then(|n| n.on_radio));
                    (on_radio != Some(false))
                        .then(|| view! { <Removal everything=false busy problem /> })
                }}
                <Removal everything=true busy problem />
                {move || {
                    problem
                        .get()
                        .map(|failure| {
                            let text = match failure {
                                Failure::Rejected(ErrorCode::RadioRefused) => t!(i18n, remove_refused).into_any(),
                                Failure::Rejected(ErrorCode::RadioDidNotAnswer) => t!(i18n, remove_no_answer).into_any(),
                                Failure::Rejected(ErrorCode::RadioNotConnected) => t!(i18n, node_not_connected).into_any(),
                                _ => t!(i18n, error_failed).into_any(),
                            };
                            view! { <p class="problem">{text}</p> }
                        })
                }}
            </div>
        </Drawer>
    }
}

/// One way to forget the node: what it does, and doing it, once more said
/// sure.
#[component]
fn Removal(
    everything: bool,
    busy: RwSignal<bool>,
    problem: RwSignal<Option<Failure>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let sure = RwSignal::new(false);
    let remove = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        problem.set(None);
        spawn_local(async move {
            let done = daemon::remove_node(radio, asking.num, everything).await;
            busy.set(false);
            sure.set(false);
            match done {
                // The page goes as the node does, as every client hears.
                Ok(()) => live.removing.set(false),
                Err(failure) => problem.set(Some(failure)),
            }
        });
    };
    let (title, text) = if everything {
        (
            t!(i18n, remove_all).into_any(),
            t!(i18n, remove_all_text).into_any(),
        )
    } else {
        (
            t!(i18n, remove_radio).into_any(),
            t!(i18n, remove_radio_text).into_any(),
        )
    };
    let button = move || {
        if everything {
            t!(i18n, remove_all_button).into_any()
        } else {
            t!(i18n, remove_radio_button).into_any()
        }
    };
    view! {
        <section class="way">
            <h3>{title}</h3>
            <p class="dim">{text}</p>
            <Show
                when=move || sure.get()
                fallback=move || {
                    view! {
                        <button class="danger-outline" disabled=move || busy.get() on:click=move |_| sure.set(true)>
                            {button}
                        </button>
                    }
                }
            >
                <div class="forget">
                    <span class="question">{t!(i18n, remove_sure)}</span>
                    <button class="danger" disabled=move || busy.get() on:click=remove>
                        {move || {
                            if busy.get() {
                                t!(i18n, remove_waiting).into_any()
                            } else {
                                t!(i18n, forget_yes).into_any()
                            }
                        }}
                    </button>
                    <button disabled=move || busy.get() on:click=move |_| sure.set(false)>
                        {t!(i18n, forget_no)}
                    </button>
                </div>
            </Show>
        </section>
    }
}

/// Every route traced to the node, the newest first, a page at a time.
#[component]
fn TracesDrawer(open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let pages = RwSignal::new(Vec::<Traceroute>::new());
    let more = RwSignal::new(false);
    let load = move |before| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(page) = daemon::traceroutes_to(radio, asking.num, before).await {
                more.set(page.len() == TRACES_PAGE);
                // The latest is no history: it is on the page, and stays.
                let skip = usize::from(before.is_none());
                pages.update(|pages| pages.extend(page.into_iter().skip(skip)));
            }
        });
    };
    // Read anew on opening, and as the routes change while open.
    Effect::new(move |_| {
        live.traceroutes.track();
        if open.get() {
            pages.set(Vec::new());
            load(None);
        }
    });
    let title = Signal::derive(move || {
        t_string!(i18n, traces_drawer_title)
            .replace("{name}", &title(live, Peer::Node { node: asking.num }))
    });
    // At its foot: forgetting what it holds, when there is any.
    let foot = ViewFn::from(move || {
        view! {
            <Show when=move || pages.with(|p| p.iter().any(|t| t.status != TracerouteStatus::Pending))>
                <Forget
                    history=History::Traceroutes
                    reload=Callback::new(move |()| {
                        pages.set(Vec::new());
                        load(None);
                    })
                />
            </Show>
        }
    });
    view! {
        <Drawer open title foot>
            <Show
                when=move || pages.with(|p| !p.is_empty())
                fallback=move || view! { <p class="nothing">{t!(i18n, traces_earlier_none)}</p> }
            >
                <For each=move || pages.get() key=|t| (t.id, t.status) let:traceroute>
                    <TraceCard traceroute />
                </For>
            </Show>
            <Show when=move || more.get()>
                <button
                    class="more"
                    on:click=move |_| load(pages.with_untracked(|p| p.last().map(|t| t.id)))
                >
                    {t!(i18n, traces_more)}
                </button>
            </Show>
        </Drawer>
    }
}

/// Everything the node was asked for, the newest first, a page at a time,
/// each with what came of it and, on opening it, its whole answer.
#[component]
fn RequestsDrawer() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let open = live.history;
    let pages = RwSignal::new(Vec::<Request>::new());
    let more = RwSignal::new(false);
    let load = move |before| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(page) = daemon::requests_page(radio, asking.num, before).await {
                more.set(page.len() == REQUESTS_PAGE);
                pages.update(|pages| pages.extend(page));
            }
        });
    };
    // Read anew on opening, and as one changes while open: one asked, or
    // one answered.
    Effect::new(move |_| {
        live.requests.track();
        if open.get() {
            pages.set(Vec::new());
            load(None);
        }
    });
    let title = Signal::derive(move || {
        t_string!(i18n, requests_drawer_title)
            .replace("{name}", &title(live, Peer::Node { node: asking.num }))
    });
    // At its foot: forgetting what it holds, when there is any.
    let foot = ViewFn::from(move || {
        view! {
            <Show when=move || pages.with(|p| p.iter().any(|r| r.status != RequestStatus::Pending))>
                <Forget
                    history=History::Requests
                    reload=Callback::new(move |()| {
                        pages.set(Vec::new());
                        load(None);
                    })
                />
            </Show>
        }
    });
    view! {
        <Drawer open title foot>
            <Show
                when=move || pages.with(|p| !p.is_empty())
                fallback=move || view! { <p class="nothing">{t!(i18n, requests_none)}</p> }
            >
                <ol class="requests">
                    <For each=move || pages.get() key=|r| (r.id, r.status) let:request>
                        <RequestRow request />
                    </For>
                </ol>
            </Show>
            <Show when=move || more.get()>
                <button
                    class="more"
                    on:click=move |_| load(pages.with_untracked(|p| p.last().map(|r| r.id)))
                >
                    {t!(i18n, traces_more)}
                </button>
            </Show>
        </Drawer>
    }
}

/// One request: what was asked, what came of it, on which channel and when;
/// opening an answered one shows its whole answer.
#[component]
fn RequestRow(request: Request) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let at = u32::try_from(request.at).unwrap_or_default();
    let error = request.error.clone();
    // A MeshCore radio gives a request up as soon as it is due, a ping
    // sooner still; a Meshtastic one, after two minutes.
    let ping = request.kind == RequestKind::Ping;
    let meshcore = untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore);
    let status = move || match request.status {
        RequestStatus::Pending => view! {
            <span class="badge syncing">{t!(i18n, request_pending)}</span>
        }
        .into_any(),
        RequestStatus::Answered => view! {
            <span class="badge ready">{t!(i18n, request_answered)}</span>
        }
        .into_any(),
        RequestStatus::Failed => {
            let why = match error.as_deref() {
                Some("TIMEOUT") if ping => t_string!(i18n, request_ping_timeout).to_owned(),
                Some("TIMEOUT") if meshcore => t_string!(i18n, request_due_timeout).to_owned(),
                Some("TIMEOUT") => t_string!(i18n, request_timeout).to_owned(),
                Some("NO_RESPONSE") => t_string!(i18n, request_no_response).to_owned(),
                Some("PKI_SEND_FAIL_PUBLIC_KEY") => t_string!(i18n, request_no_key).to_owned(),
                Some(code) => code.to_owned(),
                None => String::new(),
            };
            view! {
                <span class="badge radio-down">{t!(i18n, request_failed)} " · " {why}</span>
            }
            .into_any()
        }
    };
    let answer = request.answer.clone();
    let reply = request.reply.clone();
    let line = answer
        .as_ref()
        .map(|answer| crate::packets::summary(answer, i18n.get_locale_untracked()))
        .or_else(|| reply.as_ref().map(|reply| reply_summary(i18n, reply)));
    let detail = answer
        .and_then(|a| a.payload)
        .and_then(|payload| serde_json::to_string_pretty(&payload).ok())
        .or_else(|| reply.and_then(|reply| serde_json::to_string_pretty(&reply).ok()));
    // MeshCore's go on no channel.
    let on_channel = !meshcore_kind(request.kind)
        && untrack(|| live.selected_kind()) != Some(RadioKind::Meshcore);
    let has_detail = detail.is_some();
    let kind = request.kind;
    let clock = awaiting_clock(request.status == RequestStatus::Pending);
    view! {
        <li class="request" class:open=move || open.get()>
            <button
                class="request-line"
                disabled=!has_detail
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <span class="head">
                    <strong><KindLabel kind /></strong>
                    <span class="when">
                        {on_channel.then(|| format!("#{} · ", request.channel))}
                        {move || {
                            match clock {
                                Some(clock) => waited(clock.get(), at),
                                None => ago(live.now.get(), at),
                            }
                        }}
                    </span>
                </span>
                {status}
                <span class="summary">{line}</span>
            </button>
            <Show when=move || open.get()>
                <pre class="payload mono">{detail.clone()}</pre>
            </Show>
        </li>
    }
}

/// What a node answered with no packet, in a line.
fn reply_summary(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    reply: &meshctl_api::RequestReply,
) -> String {
    use meshctl_api::RequestReply;
    match reply {
        RequestReply::Regions {
            regions, unscoped, ..
        } => {
            let mut names: Vec<String> = regions.iter().map(|name| format!("#{name}")).collect();
            if *unscoped {
                names.insert(0, t_string!(i18n, regions_unscoped).to_owned());
            }
            if names.is_empty() {
                t_string!(i18n, regions_nothing).to_owned()
            } else {
                names.join(", ")
            }
        }
        RequestReply::Status(status) => format!(
            "{:.2} V · {} · {} dBm",
            status.voltage,
            duration(u64::from(status.uptime)),
            status.noise_floor
        ),
        RequestReply::Telemetry { readings } => readings
            .iter()
            .map(|r| reading_words(i18n, r))
            .collect::<Vec<_>>()
            .join(" · "),
        RequestReply::Neighbors { total, nodes } => t_string!(i18n, neighbors_told)
            .replace("{n}", &nodes.len().to_string())
            .replace("{total}", &total.to_string()),
        RequestReply::AccessList { nodes } => {
            t_string!(i18n, access_list_told).replace("{n}", &nodes.len().to_string())
        }
        RequestReply::Owner { firmware, name, .. } if firmware.is_empty() => name.clone(),
        RequestReply::Owner { firmware, name, .. } => format!("{name} · {firmware}"),
        RequestReply::Ping {
            round_trip_ms,
            snr_there,
            snr_back,
        } => format!("{round_trip_ms} ms · ↑ {snr_there:.1} dB · ↓ {snr_back:.1} dB"),
        RequestReply::Series { readings, .. } => readings
            .iter()
            .map(|r| series_words(i18n, r))
            .collect::<Vec<_>>()
            .join(" · "),
        RequestReply::Route { there, back } => t_string!(i18n, route_found)
            .replace("{there}", &there.len().to_string())
            .replace("{back}", &back.len().to_string()),
    }
}

/// The regions a MeshCore repeater or room repeats on, as it last said:
/// asked of it, as it says them to no one otherwise.
#[component]
fn Regions() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || {
        live.requests.with(|requests| {
            requests
                .iter()
                .filter(|r| r.node == asking.num && r.kind == RequestKind::Regions)
                .filter(|r| r.status == RequestStatus::Answered)
                .max_by_key(|r| r.answered_at)
                .and_then(|r| Some((r.reply.clone()?, r.answered_at?)))
        })
    };
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((
            meshctl_api::RequestReply::Regions {
                regions,
                unscoped,
                clock,
            },
            answered,
        )) = last()
        else {
            return view! { <p class="nothing">{t!(i18n, regions_none)}</p> }.into_any();
        };
        let nothing = regions.is_empty() && !unscoped;
        // How far off its clock was, as it answered.
        let off = clock.map(|clock| answered - clock);
        let clock_words = off.map(|off| {
            if off.abs() < 300 {
                t_string!(i18n, regions_clock_right).to_owned()
            } else if off > 0 {
                t_string!(i18n, regions_clock_behind).replace("{d}", &duration(off.unsigned_abs()))
            } else {
                t_string!(i18n, regions_clock_ahead).replace("{d}", &duration(off.unsigned_abs()))
            }
        });
        let skewed = off.is_some_and(|off| off.abs() >= 300);
        view! {
            <div class="flag-chips regions">
                {unscoped.then(|| view! { <span class="choice on">{t!(i18n, regions_unscoped)}</span> })}
                {regions
                    .into_iter()
                    .map(|name| view! { <span class="choice on">"#" {name}</span> })
                    .collect_view()}
            </div>
            {nothing.then(|| view! { <p class="nothing">{t!(i18n, regions_nothing)}</p> })}
            <p class="hint dim">{t!(i18n, regions_hint)}</p>
            <dl class="facts compact">
                {clock_words.map(|words| fact(
                    t!(i18n, regions_clock),
                    view! { <span class:warn=skewed>{words}</span> },
                ))}
            </dl>
        }
        .into_any()
    };
    move || {
        view! {
            <Section
                title=t!(i18n, node_regions).into_any()
                at=at.get()
                asked=Some(Asked::Request(RequestKind::Regions))
                class="regions-section"
            >
                {body}
            </Section>
        }
    }
}

/// The last answer a node gave to a kind of request, and when.
fn last_reply(live: Live, num: u32, kind: RequestKind) -> Option<(meshctl_api::RequestReply, i64)> {
    live.requests.with(|requests| {
        requests
            .iter()
            .filter(|r| r.node == num && r.kind == kind && r.status == RequestStatus::Answered)
            .max_by_key(|r| r.answered_at)
            .and_then(|r| Some((r.reply.clone()?, r.answered_at?)))
    })
}

/// A sensor's reading, in words: what, how much, and of which sensor when
/// not the node's own.
/// What a kind of reading is called, its unit and how many decimals it is
/// told with; none for a place or a kind not known.
fn measure(i18n: I18nContext<Locale>, kind: &str) -> Option<(&'static str, &'static str, usize)> {
    Some(match kind {
        "voltage" => (t_string!(i18n, reading_voltage), "V", 2),
        "temperature" => (t_string!(i18n, reading_temperature), "°C", 1),
        "humidity" => (t_string!(i18n, reading_humidity), "%", 0),
        "pressure" => (t_string!(i18n, reading_pressure), "hPa", 1),
        "current" => (t_string!(i18n, reading_current), "A", 3),
        "power" => (t_string!(i18n, reading_power), "W", 0),
        "illuminance" => (t_string!(i18n, reading_illuminance), "lux", 0),
        "altitude" => (t_string!(i18n, reading_altitude), "m", 0),
        "percentage" => (t_string!(i18n, reading_percentage), "%", 0),
        "energy" => (t_string!(i18n, reading_energy), "kWh", 3),
        "distance" => (t_string!(i18n, reading_distance), "m", 3),
        _ => return None,
    })
}

/// A reading's name, and its sensor's when not the node's own.
fn reading_name(i18n: I18nContext<Locale>, kind: &str, channel: u32) -> String {
    let name = match measure(i18n, kind) {
        Some((name, _, _)) => name.to_owned(),
        None if kind == "place" => t_string!(i18n, reading_place).to_owned(),
        None => kind.to_owned(),
    };
    if channel == 1 {
        name
    } else {
        format!(
            "{name} ({})",
            t_string!(i18n, reading_sensor).replace("{n}", &channel.to_string())
        )
    }
}

/// A value of a kind of reading with its unit, as many decimals as it is
/// told with.
fn reading_value(i18n: I18nContext<Locale>, kind: &str, value: f64) -> String {
    match measure(i18n, kind) {
        Some((_, unit, decimals)) => format!("{value:.decimals$} {unit}"),
        None => format!("{value}"),
    }
}

/// What a sensor kept of a reading, in words: its name, from its lowest to
/// its highest, and its mean.
fn series_words(i18n: I18nContext<Locale>, reading: &meshctl_api::SeriesReading) -> String {
    format!(
        "{} {} – {} ({} {})",
        reading_name(i18n, &reading.kind, reading.channel),
        reading_value(i18n, &reading.kind, reading.min),
        reading_value(i18n, &reading.kind, reading.max),
        t_string!(i18n, series_mean).to_lowercase(),
        reading_value(i18n, &reading.kind, reading.mean),
    )
}

fn reading_words(i18n: I18nContext<Locale>, reading: &meshctl_api::SensorReading) -> String {
    let name = reading_name(i18n, &reading.kind, reading.channel);
    let values = &reading.values;
    let value = match reading.kind.as_str() {
        "place" => format!(
            "{:.5}, {:.5}",
            values.first().copied().unwrap_or_default(),
            values.get(1).copied().unwrap_or_default()
        ),
        kind if measure(i18n, kind).is_some() => values
            .first()
            .map_or_else(String::new, |v| reading_value(i18n, kind, *v)),
        _ => values
            .iter()
            .map(|v| format!("{v}"))
            .collect::<Vec<_>>()
            .join(", "),
    };
    format!("{name} {value}")
}

/// How a MeshCore repeater or room says it is doing, asked of it.
#[component]
fn RemoteStatus() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::Status);
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((meshctl_api::RequestReply::Status(s), _)) = last() else {
            return view! { <p class="nothing">{t!(i18n, remote_status_none)}</p> }.into_any();
        };
        let packets = |all: u32, flood: u32, direct: u32| {
            t_string!(i18n, remote_packets)
                .replace("{all}", &all.to_string())
                .replace("{flood}", &flood.to_string())
                .replace("{direct}", &direct.to_string())
        };
        view! {
            <dl class="facts">
                {fact(t!(i18n, remote_voltage), format!("{:.2} V", s.voltage))}
                {fact(t!(i18n, remote_uptime), duration(u64::from(s.uptime)))}
                {fact(t!(i18n, remote_noise), format!("{} dBm", s.noise_floor))}
                {fact(
                    t!(i18n, remote_last_heard),
                    format!("{} dBm · {:.1} dB", s.last_rssi, s.last_snr),
                )}
                {fact(t!(i18n, remote_received), packets(s.received, s.received_flood, s.received_direct))}
                {fact(t!(i18n, remote_sent), packets(s.sent, s.sent_flood, s.sent_direct))}
                {fact(
                    t!(i18n, remote_duplicates),
                    format!("{} · {}", s.flood_duplicates, s.direct_duplicates),
                )}
                {fact(t!(i18n, remote_airtime), duration(u64::from(s.airtime)))}
                {s.receive_airtime.map(|t| fact(t!(i18n, remote_receive_airtime), duration(u64::from(t))))}
                {fact(t!(i18n, remote_queue), s.queue.to_string())}
                {s.receive_errors.map(|e| fact(t!(i18n, remote_receive_errors), e.to_string()))}
                {(s.errors != 0).then(|| fact(t!(i18n, remote_errors), format!("{:#06x}", s.errors)))}
                {s.posted.map(|p| fact(t!(i18n, remote_posted), p.to_string()))}
                {s.pushed.map(|p| fact(t!(i18n, remote_pushed), p.to_string()))}
            </dl>
        }
        .into_any()
    };
    move || {
        view! {
            <Section
                title=t!(i18n, remote_status).into_any()
                at=at.get()
                asked=Some(Asked::Request(RequestKind::Status))
            >
                {body}
            </Section>
        }
    }
}

/// What a MeshCore node's sensors read, asked of it.
#[component]
fn RemoteTelemetry() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::Telemetry);
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((meshctl_api::RequestReply::Telemetry { readings }, _)) = last() else {
            return view! { <p class="nothing">{t!(i18n, remote_telemetry_none)}</p> }.into_any();
        };
        view! {
            <ul class="readings">
                {readings
                    .iter()
                    .map(|r| view! { <li>{reading_words(i18n, r)}</li> })
                    .collect_view()}
            </ul>
            <p class="hint dim">{t!(i18n, remote_telemetry_hint)}</p>
        }
        .into_any()
    };
    move || {
        view! {
            <Section
                title=t!(i18n, remote_telemetry).into_any()
                at=at.get()
                asked=Some(Asked::Request(RequestKind::Telemetry))
            >
                {body}
            </Section>
        }
    }
}

/// How far back a sensor is asked what it kept: an hour, a day, a week.
const SERIES_PERIODS: [u32; 3] = [3600, 24 * 3600, 7 * 24 * 3600];

/// What a MeshCore sensor kept of its readings over a time, asked of it: the
/// lowest, the mean and the highest of each, over the last hour, day or
/// week, picked before asking.
#[component]
fn RemoteSeries() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let period = RwSignal::new(SERIES_PERIODS[1]);
    let last = move || last_reply(live, asking.num, RequestKind::Series);
    let pending = move || {
        live.requests.with(|requests| {
            requests.iter().any(|r| {
                r.node == asking.num
                    && r.kind == RequestKind::Series
                    && r.status == RequestStatus::Pending
            })
        })
    };
    let busy = move || pending() || live.logging_in_to(asking.num) || !connected(live);
    // When the latest one asked that ended came to nothing, if it did.
    let lost_at = move || {
        live.requests.with(|requests| {
            requests
                .iter()
                .filter(|r| {
                    r.node == asking.num
                        && r.kind == RequestKind::Series
                        && r.status != RequestStatus::Pending
                })
                .max_by_key(|r| r.id)
                .filter(|r| r.status == RequestStatus::Failed)
                .map(|r| r.answered_at.unwrap_or(r.at))
        })
    };
    let lost = move || lost_at().is_some();
    let periods = move || -> Vec<crate::preferences::Choice> {
        SERIES_PERIODS
            .into_iter()
            .map(|seconds| {
                let label = move || match seconds {
                    3600 => t_string!(i18n, series_hour).to_owned(),
                    86_400 => t_string!(i18n, series_day).to_owned(),
                    _ => t_string!(i18n, series_week).to_owned(),
                };
                (
                    Signal::derive(move || period.get() == seconds),
                    Box::new(move || period.set(seconds)) as Box<dyn Fn() + Send + Sync>,
                    Signal::derive(label),
                )
            })
            .collect()
    };
    let over = move |seconds: u32| match seconds {
        3600 => t_string!(i18n, series_over_hour).to_owned(),
        86_400 => t_string!(i18n, series_over_day).to_owned(),
        604_800 => t_string!(i18n, series_over_week).to_owned(),
        other => t_string!(i18n, series_over).replace("{n}", &other.to_string()),
    };
    // Asked anew: the last one's figures dashed out meanwhile, under the
    // time now asked, as like ones coming would seem none had.
    let body = move || {
        let Some((
            meshctl_api::RequestReply::Series {
                seconds, readings, ..
            },
            _,
        )) = last()
        else {
            return view! { <p class="nothing">{t!(i18n, series_none)}</p> }.into_any();
        };
        let lost = lost();
        let dashed = pending() || live.logging_in_to(asking.num) || lost;
        let seconds = if dashed { period.get() } else { seconds };
        let shown = move |text: String| {
            if dashed {
                view! { <span class="dim">"—"</span> }.into_any()
            } else {
                text.into_any()
            }
        };
        if readings.is_empty() && !dashed {
            return view! { <p class="nothing">{over(seconds)} ": " {t!(i18n, series_empty)}</p> }
                .into_any();
        }
        let rows = readings
            .into_iter()
            .map(|r| {
                view! {
                    <tr>
                        <th scope="row">{reading_name(i18n, &r.kind, r.channel)}</th>
                        <td>{shown(reading_value(i18n, &r.kind, r.min))}</td>
                        <td>{shown(reading_value(i18n, &r.kind, r.mean))}</td>
                        <td>{shown(reading_value(i18n, &r.kind, r.max))}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <table class="series">
                <caption>{over(seconds)}</caption>
                <thead>
                    <tr>
                        <th></th>
                        <th scope="col">{t!(i18n, series_min)}</th>
                        <th scope="col">{t!(i18n, series_mean)}</th>
                        <th scope="col">{t!(i18n, series_max)}</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        }
        .into_any()
    };
    move || {
        let at = lost_at().or_else(|| last().map(|(_, at)| at));
        view! {
            <Section title=t!(i18n, remote_series).into_any() at class="series-section">
                <div class="series-ask">
                    {crate::preferences::pills(periods())}
                    <button type="button" class="ask" disabled=busy on:click=move |_| {
                        live.ask_series(asking.num, period.get_untracked());
                    }>
                        {move || {
                            if live.logging_in_to(asking.num) {
                                t!(i18n, room_logging_in).into_any()
                            } else if pending() {
                                t!(i18n, node_asking).into_any()
                            } else {
                                t!(i18n, node_ask_button).into_any()
                            }
                        }}
                    </button>
                </div>
                {body}
                {move || {
                    (lost() && !pending())
                        .then(|| view! { <p class="hint dim">{t!(i18n, series_lost)}</p> })
                }}
                <p class="hint dim">{t!(i18n, series_hint)}</p>
            </Section>
        }
    }
}

/// The nodes a MeshCore repeater or room hears straight, asked of it: each
/// a click away from its page, when known.
#[component]
fn RemoteNeighbors() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::Neighbors);
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((meshctl_api::RequestReply::Neighbors { total, nodes }, _)) = last() else {
            return view! { <p class="nothing">{t!(i18n, remote_neighbors_none)}</p> }.into_any();
        };
        let more = (u32::try_from(nodes.len()).unwrap_or(0) < total).then(|| {
            t_string!(i18n, neighbors_told)
                .replace("{n}", &nodes.len().to_string())
                .replace("{total}", &total.to_string())
        });
        view! {
            <ul class="neighbours">
                {nodes
                    .into_iter()
                    .map(|n| {
                        let node = n.node;
                        let known = live.nodes.with_untracked(|nodes| nodes.contains_key(&node));
                        let name = crate::chat::node_name(live, node);
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="link"
                                    disabled=!known
                                    on:click=move |_| live.open_node(node)
                                >
                                    {name}
                                </button>
                                <span class="snr">
                                    {format!("{:.1} dB · ", n.snr)}
                                    {t_string!(i18n, node_ago).replace("{ago}", &duration(u64::from(n.seconds_ago)))}
                                </span>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
            {more.map(|more| view! { <p class="hint dim">{more}</p> })}
        }
        .into_any()
    };
    move || {
        view! {
            <Section
                title=t!(i18n, remote_neighbors).into_any()
                at=at.get()
                asked=Some(Asked::Request(RequestKind::Neighbors))
            >
                {body}
            </Section>
        }
    }
}

/// Who runs a MeshCore node, as it says, asked of it.
#[component]
fn RemoteOwner() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::Owner);
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((
            meshctl_api::RequestReply::Owner {
                firmware,
                name,
                owner,
            },
            _,
        )) = last()
        else {
            return view! { <p class="nothing">{t!(i18n, remote_owner_none)}</p> }.into_any();
        };
        view! {
            <dl class="facts">
                {fact(t!(i18n, remote_owner_name), name)}
                {(!firmware.is_empty()).then(|| fact(t!(i18n, remote_owner_firmware), firmware))}
                {(!owner.is_empty()).then(|| fact(t!(i18n, remote_owner_owner), owner))}
            </dl>
        }
        .into_any()
    };
    move || {
        view! {
            <Section
                title=t!(i18n, remote_owner).into_any()
                at=at.get()
                asked=Some(Asked::Request(RequestKind::Owner))
            >
                {body}
            </Section>
        }
    }
}

/// A MeshCore node heard straight, pinged: how long there and back, and how
/// each end heard the other. Only for one heard straight, or pinged before.
#[component]
fn RemotePing() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::Ping);
    // When the latest ping that ended came to nothing: its figures, the last
    // one's that came back, are no more what the node is now.
    let lost = move || {
        live.requests.with(|requests| {
            requests
                .iter()
                .filter(|r| {
                    r.node == asking.num
                        && r.kind == RequestKind::Ping
                        && r.status != RequestStatus::Pending
                })
                .max_by_key(|r| r.id)
                .filter(|r| r.status == RequestStatus::Failed)
                .map(|r| r.answered_at.unwrap_or(r.at))
        })
    };
    let at = Signal::derive(move || lost().or_else(|| last().map(|(_, at)| at)));
    let direct = move || {
        live.nodes
            .with(|nodes| nodes.get(&asking.num).and_then(|n| n.hops_away))
            == Some(0)
    };
    // Pinged again: the last one's figures dashed out meanwhile, as like
    // ones coming would seem none had.
    let pinging = move || {
        live.requests.with(|requests| {
            requests.iter().any(|r| {
                r.node == asking.num
                    && r.kind == RequestKind::Ping
                    && r.status == RequestStatus::Pending
            })
        })
    };
    let body = move || {
        let result = last().and_then(|(reply, _)| match reply {
            meshctl_api::RequestReply::Ping {
                round_trip_ms,
                snr_there,
                snr_back,
            } => Some((round_trip_ms, snr_there, snr_back)),
            _ => None,
        });
        let pinging = pinging();
        let lost = lost().is_some();
        let dashed = pinging || lost;
        let value = move |shown: AnyView| {
            if dashed {
                view! { <span class="dim">"—"</span> }.into_any()
            } else {
                shown
            }
        };
        view! {
            {result.map(|(ms, there, back)| view! {
                <dl class="facts compact three">
                    {fact(t!(i18n, ping_round_trip), value(format!("{ms} ms").into_any()))}
                    // Coloured as how well each way was heard, as a trace's hops.
                    {fact(t!(i18n, ping_there), value(snr_view(there).into_any()))}
                    {fact(t!(i18n, ping_back), value(snr_view(back).into_any()))}
                </dl>
            })}
            {(lost && !pinging).then(|| view! { <p class="hint dim">{t!(i18n, ping_lost)}</p> })}
            {(!direct()).then(|| view! { <p class="hint dim">{t!(i18n, ping_only_direct)}</p> })}
        }
    };
    // Nothing to show of one not heard straight and never pinged: not
    // there. One pinged before, as it was heard straight, keeps its last.
    move || {
        (direct() || at.get().is_some()).then(|| {
            view! {
                <Section
                    title=t!(i18n, remote_ping).into_any()
                    at=at.get()
                    asked=direct().then_some(Asked::Request(RequestKind::Ping))
                >
                    {body}
                </Section>
            }
        })
    }
}

/// The radio's login to a MeshCore repeater, room or sensor: as what, a
/// password to log in with, kept once right, and forgetting it. What is
/// asked of it logs in as a guest first, if the radio has not since it
/// connected.
#[component]
fn NodeAccessSection() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let login = expect_context::<NodeLoginState>().0;
    let busy = RwSignal::new(false);
    let problem = RwSignal::new(None::<Failure>);
    let password = RwSignal::new(String::new());
    let read = move || {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(read) = daemon::node_login(radio, asking.num).await {
                login.try_set(Some(read));
            }
        });
    };
    // Read as the page opens, and again as requests come back, as one asked
    // logs in first.
    Effect::new(move |_| {
        live.requests.track();
        live.synced.track();
        read();
    });
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let given = password.get_untracked();
        busy.set(true);
        problem.set(None);
        spawn_local(async move {
            let done =
                daemon::log_in_node(radio, asking.num, (!given.is_empty()).then_some(given)).await;
            busy.try_set(false);
            match done {
                Ok(read) => {
                    login.try_set(Some(read));
                    password.try_set(String::new());
                    live.login_failed.update(|l| {
                        l.remove(&asking.num);
                    });
                }
                Err(failure) => {
                    problem.try_set(Some(failure));
                }
            }
        });
    };
    let forget = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(read) = daemon::forget_node_password(radio, asking.num).await {
                login.try_set(Some(read));
            }
        });
    };
    // Logging in now, as asked for something: how long it is waited for
    // yet, counted down.
    let entering = move || {
        asking.clock.track();
        live.logging_in
            .with(|l| l.get(&asking.num).copied())
            .map(|(until, _)| {
                let left = ((until - js_sys::Date::now()) / 1000.0).ceil().max(0.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let left = left as u32;
                t_string!(i18n, access_entering).replace("{s}", &left.to_string())
            })
    };
    let access = move || {
        if let Some(entering) = entering() {
            return entering;
        }
        let access = login.with(|l| l.as_ref().and_then(|l| l.access));
        match access {
            Some(meshctl_api::NodeAccess::Admin) => t_string!(i18n, access_admin),
            Some(meshctl_api::NodeAccess::ReadWrite) => t_string!(i18n, access_read_write),
            Some(meshctl_api::NodeAccess::ReadOnly) => t_string!(i18n, access_guest),
            None => t_string!(i18n, access_out),
        }
        .to_owned()
    };
    let remembered = move || login.with(|l| l.as_ref().is_some_and(|l| l.remembered));
    // Why it did not go in: here, or as asked for something, until it goes
    // in; not while it tries again.
    let problem_words = move || {
        let asked = live.login_failed.with(|l| l.get(&asking.num).copied());
        let logged = login.with(|l| l.as_ref().is_some_and(|l| l.access.is_some()));
        let failure = problem.get().or(asked.filter(|_| !logged));
        failure
            .filter(|_| entering().is_none())
            .map(|failure| match failure {
                Failure::Rejected(ErrorCode::WrongPassword) => t_string!(i18n, room_wrong_password),
                Failure::Rejected(ErrorCode::RadioDidNotAnswer) => {
                    t_string!(i18n, access_no_answer)
                }
                Failure::Rejected(ErrorCode::RadioNotConnected) => {
                    t_string!(i18n, node_not_connected)
                }
                _ => t_string!(i18n, error_failed),
            })
    };
    view! {
        <Section title=t!(i18n, access_title).into_any()>
            <dl class="facts compact">
                {fact(t!(i18n, access_as), access)}
            </dl>
            <form class="access-form" on:submit=submit>
                <input
                    type="text"
                    class="secret"
                    maxlength="15"
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    data-lpignore="true"
                    data-1p-ignore="true"
                    data-bwignore="true"
                    data-form-type="other"
                    placeholder=move || t_string!(i18n, access_password)
                    aria-label=move || t_string!(i18n, access_password)
                    prop:value=move || password.get()
                    on:input=move |ev| password.set(event_target_value(&ev))
                />
                <button type="submit" disabled=move || busy.get()>
                    {move || {
                        if busy.get() {
                            t!(i18n, room_logging_in).into_any()
                        } else {
                            t!(i18n, room_log_in).into_any()
                        }
                    }}
                </button>
            </form>
            {move || problem_words().map(|w| view! { <p class="problem">{w}</p> })}
            <p class="hint dim">{t!(i18n, access_hint)}</p>
            <Show when=remembered>
                <button type="button" class="link" on:click=forget>
                    {t!(i18n, room_forget_password)}
                </button>
            </Show>
        </Section>
    }
}

/// Whether the radio is logged in to the node of the page as its admin.
fn admin_here() -> bool {
    expect_context::<NodeLoginState>()
        .0
        .with(|l| l.as_ref().and_then(|l| l.access))
        == Some(meshctl_api::NodeAccess::Admin)
}

/// Who may log in to a MeshCore node, and as what: its admin's to ask.
#[component]
fn AccessListSection() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let last = move || last_reply(live, asking.num, RequestKind::AccessList);
    let at = Signal::derive(move || last().map(|(_, at)| at));
    let body = move || {
        let Some((meshctl_api::RequestReply::AccessList { nodes }, _)) = last() else {
            return view! { <p class="nothing">{t!(i18n, access_list_none)}</p> }.into_any();
        };
        view! {
            <ul class="neighbours">
                {nodes
                    .into_iter()
                    .map(|listed| {
                        let node = listed.node;
                        let known = live.nodes.with_untracked(|nodes| nodes.contains_key(&node));
                        let access = match listed.access {
                            meshctl_api::NodeAccess::Admin => t_string!(i18n, access_admin),
                            meshctl_api::NodeAccess::ReadWrite => t_string!(i18n, access_read_write),
                            meshctl_api::NodeAccess::ReadOnly => t_string!(i18n, access_guest),
                        };
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="link"
                                    disabled=!known
                                    on:click=move |_| live.open_node(node)
                                >
                                    {crate::chat::node_name(live, node)}
                                </button>
                                <span class="snr">{access}</span>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };
    move || {
        admin_here().then(|| {
            view! {
                <Section
                    title=t!(i18n, access_list).into_any()
                    at=at.get()
                    asked=Some(Asked::Request(RequestKind::AccessList))
                >
                    {body}
                </Section>
            }
        })
    }
}

/// Commands a console takes that are past undoing, or lock its admin out:
/// asked again before they go.
const RISKY_COMMANDS: [&str; 8] = [
    "reboot",
    "erase",
    "shutdown",
    "poweroff",
    "start ota",
    "password",
    "prv.key",
    "clkreboot",
];

/// The console of a MeshCore node, for its admin: a command, as its
/// firmware's CLI takes it, and what it answered, as it comes.
#[component]
fn ConsoleSection(
    /// The radio's own, whose console needs no login.
    #[prop(optional)]
    own: bool,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let asking = expect_context::<Asking>();
    let command = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let problem = RwSignal::new(None::<Failure>);
    let lines = move || {
        let radio = live.selected.get();
        live.console.with(|lines| {
            lines
                .iter()
                .filter(|l| Some(l.radio) == radio && l.node == asking.num)
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let send = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let text = command.get_untracked().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let risky = RISKY_COMMANDS
            .iter()
            .any(|risky| text == *risky || text.starts_with(&format!("{risky} ")));
        if risky {
            let sure = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        &t_string!(i18n, console_risky).replace("{command}", &text),
                    )
                    .ok()
                })
                .unwrap_or(false);
            if !sure {
                return;
            }
        }
        busy.set(true);
        // In the log as it goes, before what answers it, which may come
        // before the daemon says it went, as the radio's own console does.
        live.console.update(|lines| {
            lines.push(crate::app::ConsoleLine {
                radio,
                node: asking.num,
                mine: true,
                text: text.clone(),
                at: i64::from(crate::app::now()),
            });
        });
        problem.set(None);
        spawn_local(async move {
            let sent = daemon::node_command(radio, asking.num, &text).await;
            busy.try_set(false);
            match sent {
                Ok(()) => {
                    command.try_set(String::new());
                }
                // Not sent: out of the log again.
                Err(failure) => {
                    live.console.update(|lines| {
                        if let Some(at) = lines.iter().rposition(|l| {
                            l.mine && l.radio == radio && l.node == asking.num && l.text == text
                        }) {
                            lines.remove(at);
                        }
                    });
                    problem.try_set(Some(failure));
                }
            }
        });
    };
    let problem_words = move || {
        problem.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::NotAdmin) => t_string!(i18n, console_not_admin),
            Failure::Rejected(ErrorCode::RadioDidNotAnswer) => t_string!(i18n, access_no_answer),
            Failure::Rejected(ErrorCode::RadioNotConnected) => t_string!(i18n, node_not_connected),
            _ => t_string!(i18n, error_failed),
        })
    };
    move || {
        (own || admin_here()).then(|| {
            view! {
                <Section title=t!(i18n, console_title).into_any()>
                    <ol class="console mono">
                        {move || {
                            let lines = lines();
                            if lines.is_empty() {
                                return view! {
                                    <li class="dim">
                                        {if own {
                                            t!(i18n, console_hint_own).into_any()
                                        } else {
                                            t!(i18n, console_hint).into_any()
                                        }}
                                    </li>
                                }
                                .into_any();
                            }
                            lines
                                .into_iter()
                                .map(|line| {
                                    // An answer without the "> " its firmware puts before
                                    // what it gets, which reads as one of the commands.
                                    let text = if line.mine {
                                        line.text
                                    } else {
                                        line.text
                                            .strip_prefix("> ")
                                            .map_or_else(|| line.text.clone(), str::to_owned)
                                    };
                                    view! {
                                        <li class:mine=line.mine>
                                            {line.mine.then_some("> ")}
                                            {text}
                                        </li>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </ol>
                    <form class="access-form" on:submit=send>
                        <input
                            type="text"
                            class="mono"
                            maxlength="200"
                            autocomplete="off"
                            autocapitalize="off"
                            spellcheck="false"
                            placeholder=if own { "ver, board, get name…" } else { "ver, clock, get name…" }
                            aria-label=move || t_string!(i18n, console_title)
                            prop:value=move || command.get()
                            on:input=move |ev| command.set(event_target_value(&ev))
                        />
                        <button type="submit" disabled=move || busy.get()>
                            {t!(i18n, console_send)}
                        </button>
                    </form>
                    {move || problem_words().map(|w| view! { <p class="problem">{w}</p> })}
                </Section>
            }
        })
    }
}

/// What a kind of request is called.
#[component]
fn KindLabel(kind: RequestKind) -> impl IntoView {
    let i18n = use_i18n();
    move || match kind {
        RequestKind::NodeInfo => t!(i18n, ask_node_info).into_any(),
        RequestKind::Position => t!(i18n, ask_position).into_any(),
        RequestKind::DeviceTelemetry => t!(i18n, ask_device).into_any(),
        RequestKind::EnvironmentTelemetry => t!(i18n, ask_environment).into_any(),
        RequestKind::PowerTelemetry => t!(i18n, ask_power).into_any(),
        RequestKind::LocalStats => t!(i18n, ask_local_stats).into_any(),
        RequestKind::Neighbors => t!(i18n, ask_neighbors).into_any(),
        RequestKind::Regions => t!(i18n, ask_regions).into_any(),
        RequestKind::Status => t!(i18n, ask_status).into_any(),
        RequestKind::Telemetry => t!(i18n, ask_telemetry).into_any(),
        RequestKind::Owner => t!(i18n, ask_owner).into_any(),
        RequestKind::AccessList => t!(i18n, ask_access_list).into_any(),
        RequestKind::Ping => t!(i18n, ask_ping).into_any(),
        RequestKind::Route => t!(i18n, ask_route).into_any(),
        RequestKind::Series => t!(i18n, ask_series).into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::duration;

    #[test]
    fn durations_read_in_their_two_largest_units() {
        assert_eq!(duration(42), "42 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(duration(5 * 3600 + 720), "5 h 12 min");
        assert_eq!(duration(3 * 86_400 + 4 * 3600 + 59), "3 d 4 h");
    }
}
