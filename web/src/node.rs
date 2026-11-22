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
use crate::mesh::{PIN, ago, radius, reach_words};
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

/// What the open node's requests go by: the node, and a clock by the second
/// for how long the radio keeps from sending.
#[derive(Clone, Copy)]
struct Asking {
    num: u32,
    clock: RwSignal<u32>,
}

/// What a button of the page sends: a request of a kind, or a traceroute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
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

/// A button of the page: what it sends, and where it is, as two may send
/// the same (its node info, over all while its name is not known, and in
/// its section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ButtonId {
    asked: Asked,
    place: &'static str,
}

/// Which button's channel menu is open, if any.
#[derive(Clone, Copy)]
struct OpenMenu(RwSignal<Option<ButtonId>>);

/// Which button was pressed last: the one that shows the radio's wait
/// filling up, while the others it holds back are only off.
#[derive(Clone, Copy)]
struct Pressed(RwSignal<Option<ButtonId>>);

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
    // One channel menu open at a time, closed by a click anywhere else.
    let menu = RwSignal::new(None::<ButtonId>);
    provide_context(OpenMenu(menu));
    provide_context(Pressed(RwSignal::new(None)));
    let closing = window_event_listener(leptos::ev::click, move |_| menu.set(None));
    on_cleanup(move || closing.remove());
    let drawer = RwSignal::new(false);
    let latest = move || live.latest.get().unwrap_or_default();
    let is_own = Memo::new(move |_| own(live, num));
    // Two columns, headed by what is looked at most: where it is, and the
    // routes to it: what comes of its own accord on the left, what is asked
    // for on the right, the least likely to answer last.
    let heard = move || {
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
                view! {
                    {placed.then(|| view! {
                        <PositionSection at=latest.position.as_ref().map(|p| p.at) />
                    })}
                    <Device packet=latest.device.clone() />
                    <Stats packet=latest.local_stats.clone() />
                    <Sensors latest own=true />
                }
            }}
            <Shortcuts />
        }
        .into_any()
    };
    view! {
        <div class="node-page" class:own-page=is_own>
            <Checks />
            <Tiles num />
            <div class="node-sections" class:balanced=is_own>
                {move || if is_own.get() { mine() } else { heard() }}
            </div>
            // Eye-catching, but long: below what is looked at most.
            {move || view! { <crate::charts::Telemetry num own=is_own.get() /> }}
            {move || {
                (!is_own.get()).then(|| view! {
                    <TracesDrawer open=drawer />
                    <RequestsDrawer />
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

/// Whether the selected radio can ask nodes for anything yet: MeshCore's
/// requests are still to come.
fn supported(live: Live) -> bool {
    live.selected_kind() != Some(RadioKind::Meshcore)
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
        let note = if !supported(live) {
            Some(t!(i18n, meshcore_not_yet).into_any())
        } else if !connected(live) {
            Some(t!(i18n, node_not_connected).into_any())
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
    let Pressed(pressed) = expect_context::<Pressed>();
    let me = ButtonId { asked, place };
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
    let wait = move || {
        asked
            .pace()
            .map_or(0, |pace| live.wait(pace, asking.clock.get()))
    };
    // Not to be sent without its key: the radio refuses to.
    let keyless = move || asked.private() && needs_key(live, asking.num);
    let off = move || !connected(live) || pending() || wait() > 0 || keyless();
    let send = move |channel: u32| {
        pressed.set(Some(me));
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
    let has_menu = move || by_channel() && channels().len() > 1;
    let open = move || menu.get() == Some(me);
    // Asking, said by the button pressed; by every one that sends it when
    // it was asked from elsewhere.
    let asking_here = move || {
        pending()
            && pressed
                .get()
                .is_none_or(|pressed| pressed == me || pressed.asked != asked)
    };
    let label = move || {
        if asking_here() {
            t_string!(i18n, node_asking).to_owned()
        } else if pressed.get() == Some(me) && wait() > 0 {
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
        (supported(live) && !own(live, asking.num)).then(|| {
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
                                .filter(|_| pressed.get() == Some(me))
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
    let window: u32 = match kind {
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
                        <span class="mono key">{key}</span>
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
        let preset = local.modem_preset.as_deref().map_or_else(
            || t_string!(i18n, own_custom_lora).to_owned(),
            |p| crate::names::preset(locale, p),
        );
        Some(view! {
            <dl class="facts">
                {fact(t!(i18n, nodes_id), view! { <span class="mono">{local.id.clone()}</span> })}
                {local.firmware_version.clone().map(|v| fact(t!(i18n, own_firmware), view! { <span class="mono">{v}</span> }))}
                {local.hw_model.as_deref().map(|hw| fact(t!(i18n, nodes_hardware), crate::mesh::model(hw)))}
                {local.role.as_deref().map(|role| fact(t!(i18n, node_role), crate::names::role(locale, role)))}
                {local.region.as_deref().map(|region| fact(t!(i18n, own_region), crate::names::region(locale, region)))}
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
                            <button
                                class="icon-button"
                                title=move || t_string!(i18n, node_on_map)
                                aria-label=move || t_string!(i18n, node_on_map)
                                inner_html=PIN
                                on:click=move |_| live.show_on_map(asking.num)
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
    view! {
        <Section title=t!(i18n, node_position).into_any() at asked=Some(Asked::Request(RequestKind::Position))>
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
    let body = move || {
        let node = live.nodes.with(|nodes| nodes.get(&asking.num).cloned())?;
        let key = match node.public_key.clone() {
            Some(key) => {
                let copied = key.clone();
                view! {
                    <span class="copyable">
                        <span class="mono key">{key}</span>
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
            }
            None => view! { <span class="dim">{t!(i18n, node_key_unknown)}</span> }.into_any(),
        };
        // How what is for it alone reaches it: telemetry and neighbours
        // asked, and messages to it.
        let direct = if licensed(live) {
            t!(i18n, node_direct_licensed).into_any()
        } else if node.public_key.is_some() {
            t!(i18n, node_direct_pki).into_any()
        } else {
            t!(i18n, node_direct_none).into_any()
        };
        Some(view! {
            <dl class="facts">
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
                <dl class="facts">
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
    Some(view! { <dl class="facts">{facts}</dl> }.into_any())
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
    Some(view! { <dl class="facts">{facts}</dl> }.into_any())
}

/// The nodes it hears directly, each opening its page if the radio knows
/// of it.
fn neighbour_list(
    i18n: I18nContext<Locale>,
    live: Live,
    packet: Option<&Packet>,
) -> Option<AnyView> {
    let info = object(packet, None)?;
    let neighbours: Vec<(u32, Option<f64>)> = info
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
    let rows = neighbours
        .into_iter()
        .map(|(num, snr)| {
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
                <li>
                    {who}
                    <span class="snr">{snr.map(|s| format!("{s:.1} dB"))}</span>
                </li>
            }
        })
        .collect_view();
    Some(view! { <ul class="neighbours">{rows}</ul> }.into_any())
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
    if own && let Some(radio) = live.selected.get_untracked() {
        spawn_local(async move {
            if let Ok(sections) = daemon::sections(radio).await {
                enabled.set(Some(modules_on(&sections)));
            }
        });
    }
    // Its words follow the language: built where that is tracked.
    move || {
        // Here, and not as the parts' properties: those are built untracked.
        let environment = environment_facts(i18n, latest.environment.as_ref());
        let power = power_facts(i18n, latest.power.as_ref());
        let neighbours = neighbour_list(i18n, live, latest.neighbors.as_ref());
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
                <dl class="facts">
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
    let line = answer
        .as_ref()
        .map(|answer| crate::packets::summary(answer, i18n.get_locale_untracked()));
    let detail = answer
        .and_then(|a| a.payload)
        .and_then(|payload| serde_json::to_string_pretty(&payload).ok());
    let has_detail = detail.is_some();
    let kind = request.kind;
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
                        "#" {request.channel} " · " {move || ago(live.now.get(), at)}
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
