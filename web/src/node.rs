// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A node's page: what is known of it, what it can be asked for, and what it
//! answered, traceroutes among it.

use crate::app::Live;
use crate::chat::title;
use crate::daemon::Failure;
use crate::empty::EmptyState;
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::{ago, radius, reach_words};
use crate::packets::summary;
use crate::traces::TraceCard;
use leptos::prelude::*;
use meshctl_api::{
    ErrorCode, LinkState, Pace, Peer, PositionSource, RadioKind, Request, RequestKind,
    RequestStatus,
};
use std::time::Duration;

#[component]
pub fn NodeView() -> impl IntoView {
    let live = expect_context::<Live>();
    move || {
        live.node.get().map(|num| {
            view! {
                <Tiles num />
                <Position num />
                <Ask num />
                <Requests />
                <Traces num />
            }
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
        let heard = node.last_heard.map(|at| {
            view! {
                <Tile label=t!(i18n, nodes_heard).into_any() value=ago(live.now.get(), at) />
            }
        });
        // Heard through others, the signal is the last relay's: only said
        // when it is the node's own.
        let reach = reach_words(i18n, &node, local).map(|words| {
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
        Some(view! { <div class="node-tiles">{heard} {reach} {power} {busy}</div> })
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
        <section class="card tile" class:low=low>
            <span class="label">{label}</span>
            <span class="value">{value}</span>
            {bar.map(|bar| {
                let width = format!("width: {:.0}%", bar.clamp(0.0, 100.0));
                view! { <span class="bar"><i style=width></i></span> }
            })}
            {sub.map(|sub| view! { <span class="sub">{sub}</span> })}
        </section>
    }
}

/// Where the node is, and how well that is known.
#[component]
fn Position(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let row = |label: AnyView, value: Option<String>| {
        value.map(|value| view! { <div><dt>{label}</dt><dd>{value}</dd></div> })
    };
    move || {
        let position = live.nodes.with(|nodes| nodes.get(&num)?.position)?;
        let altitude = position
            .altitude
            .map_or_else(String::new, |a| format!(" · {a} m"));
        let place = format!(
            "{:.5}, {:.5}{altitude}",
            position.latitude, position.longitude
        );
        let source = position.source.map(|source| {
            match source {
                PositionSource::Manual => t_string!(i18n, position_manual),
                PositionSource::Internal => t_string!(i18n, position_internal),
                PositionSource::External => t_string!(i18n, position_external),
            }
            .to_owned()
        });
        Some(view! {
            <section class="card node-details">
                <dl>
                    {row(t!(i18n, node_position).into_any(), Some(place))}
                    {row(t!(i18n, node_precision).into_any(), position.radius.map(radius))}
                    {row(t!(i18n, node_source).into_any(), source)}
                </dl>
            </section>
        })
    }
}

/// What the node can be asked for, and on which channel.
#[component]
fn Ask(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let channel = RwSignal::new(0_u32);
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect::<Vec<_>>()
        })
    };
    let connected = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    let supported = move || live.selected_kind() != Some(RadioKind::Meshcore);
    let waiting = move |kind: RequestKind| {
        live.requests.with(|requests| {
            requests
                .iter()
                .any(|r| r.kind == kind && r.status == RequestStatus::Pending)
        })
    };
    let kinds = [
        RequestKind::NodeInfo,
        RequestKind::Position,
        RequestKind::DeviceTelemetry,
        RequestKind::EnvironmentTelemetry,
        RequestKind::PowerTelemetry,
        RequestKind::LocalStats,
        RequestKind::Neighbors,
    ];
    // A clock of its own, by the second, for how long the radio keeps from
    // sending what it sent a moment ago.
    let clock = RwSignal::new(crate::app::now());
    if let Ok(ticking) =
        set_interval_with_handle(move || clock.set(crate::app::now()), Duration::from_secs(1))
    {
        on_cleanup(move || ticking.clear());
    }
    let wait = move |pace: Option<Pace>| pace.map_or(0, |pace| live.wait(pace, clock.get()));
    let countdown = move |pace: Option<Pace>| {
        let wait = wait(pace);
        (wait > 0).then(|| view! { <span class="wait">{format!("{wait} s")}</span> })
    };
    let buttons = move || {
        kinds
            .into_iter()
            .map(|kind| {
                view! {
                    <button
                        disabled=move || !connected() || waiting(kind) || (wait(kind.pace()) > 0)
                        on:click=move |_| live.ask(num, kind, channel.get_untracked())
                    >
                        <KindLabel kind />
                        {move || countdown(kind.pace())}
                    </button>
                }
            })
            .collect_view()
    };
    let problem = move || {
        if !connected() {
            return Some(t!(i18n, node_not_connected).into_any());
        }
        live.ask_error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::TooSoon) => t!(i18n, ask_too_soon).into_any(),
            Failure::Rejected(ErrorCode::InvalidChannel) => {
                t!(i18n, ask_invalid_channel).into_any()
            }
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t!(i18n, node_not_connected).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };

    view! {
        <Show
            when=supported
            fallback=move || {
                view! {
                    <EmptyState
                        icon=crate::empty::SETTINGS_OFF
                        title=t!(i18n, empty_meshcore_title).into_any()
                        text=t!(i18n, meshcore_not_yet).into_any()
                        compact=true
                    />
                }
            }
        >
            <section class="card node-ask">
                <h2>{t!(i18n, node_ask)}</h2>
                <p class="dim">{t!(i18n, node_ask_text)}</p>
                <label class="channel">
                    <span>{t!(i18n, node_channel)}</span>
                    <select on:change=move |ev| {
                        if let Ok(index) = event_target_value(&ev).parse() {
                            channel.set(index);
                        }
                    }>
                        <For each=channels key=|index| *index let:index>
                            <option value=index.to_string() selected=move || channel.get() == index>
                                {move || format!("{index} · {}", title(live, Peer::Channel { channel: index }))}
                            </option>
                        </For>
                    </select>
                </label>
                <div class="asks">
                    {buttons}
                    <button
                        disabled=move || !connected() || (wait(Some(Pace::Traceroute)) > 0)
                        on:click=move |_| live.trace(num, channel.get_untracked())
                    >
                        {t!(i18n, ask_traceroute)}
                        {move || countdown(Some(Pace::Traceroute))}
                    </button>
                </div>
                <p class="error">{problem}</p>
            </section>
        </Show>
    }
}

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

/// What the node was asked for, the newest first, and what it answered.
#[component]
fn Requests() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    view! {
        <Show when=move || live.requests.with(|r| !r.is_empty())>
            <section class="card node-requests">
                <h2>{t!(i18n, node_requests)}</h2>
                <ol>
                    <For
                        each=move || live.requests.get()
                        key=|r| (r.id, r.status)
                        let:request
                    >
                        <RequestRow request />
                    </For>
                </ol>
            </section>
        </Show>
    }
}

#[component]
fn RequestRow(request: Request) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let at = u32::try_from(request.at).unwrap_or_default();
    let status = match request.status {
        RequestStatus::Pending => view! {
            <span class="badge syncing">{t!(i18n, request_pending)}</span>
        }
        .into_any(),
        RequestStatus::Answered => view! {
            <span class="badge ready">{t!(i18n, request_answered)}</span>
        }
        .into_any(),
        RequestStatus::Failed => {
            let why = match request.error.as_deref() {
                Some("TIMEOUT") => t!(i18n, request_timeout).into_any(),
                Some("NO_RESPONSE") => t!(i18n, request_no_response).into_any(),
                Some(code) => code.to_owned().into_any(),
                None => ().into_any(),
            };
            view! {
                <span class="badge radio-down">{t!(i18n, request_failed)} " · " {why}</span>
            }
            .into_any()
        }
    };
    let answer = request.answer.clone();
    let line = answer.as_ref().map(summary);
    let detail = answer
        .and_then(|a| a.payload)
        .and_then(|payload| serde_json::to_string_pretty(&payload).ok());
    let has_detail = detail.is_some();
    view! {
        <li class="request" class:open=move || open.get()>
            <button
                class="request-line"
                disabled=!has_detail
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <strong><KindLabel kind=request.kind /></strong>
                {status}
                <span class="summary">{line}</span>
                <span class="dim when">"#" {request.channel} " · " {move || ago(live.now.get(), at)}</span>
            </button>
            <Show when=move || open.get()>
                <pre class="payload mono">{detail.clone()}</pre>
            </Show>
        </li>
    }
}

/// The routes traced to the node.
#[component]
fn Traces(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let traces = move || {
        live.traceroutes.with(|traces| {
            traces
                .iter()
                .filter(|t| t.target == num)
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    view! {
        <Show when=move || !traces().is_empty()>
            <section class="card traces">
                <h2>{t!(i18n, node_traces)}</h2>
                <For each=traces key=|t| (t.id, t.status) let:traceroute>
                    <TraceCard traceroute />
                </For>
            </section>
        </Show>
    }
}
