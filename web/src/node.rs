// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A node's page: what is known of it, what it can be asked for, and what it
//! answered, traceroutes among it.

use crate::app::Live;
use crate::chat::title;
use crate::daemon::Failure;
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::{ago, model, power, radius};
use crate::packets::summary;
use crate::traces::TraceCard;
use leptos::prelude::*;
use meshctl_api::{
    ErrorCode, LinkState, Peer, PositionSource, RadioKind, Request, RequestKind, RequestStatus,
};

#[component]
pub fn NodeView() -> impl IntoView {
    let live = expect_context::<Live>();
    move || {
        live.node.get().map(|num| {
            view! {
                <Details num />
                <Ask num />
                <Requests />
                <Traces num />
            }
        })
    }
}

/// What is known of the node.
#[component]
fn Details(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = move || live.nodes.with(|nodes| nodes.get(&num).cloned());
    let row = |label: AnyView, value: Option<String>| {
        value.map(|value| view! { <div><dt>{label}</dt><dd>{value}</dd></div> })
    };
    move || {
        let node = node();
        let hops = node.as_ref().and_then(|n| {
            if n.via_mqtt {
                Some("MQTT".to_owned())
            } else {
                n.hops_away.map(|h| {
                    if h == 0 {
                        t_string!(i18n, map_direct).to_owned()
                    } else {
                        h.to_string()
                    }
                })
            }
        });
        let signal = node.as_ref().and_then(|n| {
            let snr = n.snr.filter(|_| n.hops_away == Some(0) && !n.via_mqtt)?;
            Some(match n.rssi {
                Some(rssi) => format!("SNR {snr:.1} dB · RSSI {rssi} dBm"),
                None => format!("SNR {snr:.1} dB"),
            })
        });
        let heard = node
            .as_ref()
            .and_then(|n| n.last_heard)
            .map(|at| ago(live.now.get(), at));
        let battery = node
            .as_ref()
            .and_then(|n| power(n.battery_level, n.voltage, t_string!(i18n, power_external)));
        let position = node.as_ref().and_then(|n| {
            let p = n.position.as_ref()?;
            let altitude = p.altitude.map_or_else(String::new, |a| format!(" · {a} m"));
            Some(format!("{:.5}, {:.5}{altitude}", p.latitude, p.longitude))
        });
        let precision = node
            .as_ref()
            .and_then(|n| n.position.as_ref()?.radius)
            .map(radius);
        let source = node
            .as_ref()
            .and_then(|n| n.position.as_ref()?.source)
            .map(|source| {
                match source {
                    PositionSource::Manual => t_string!(i18n, position_manual),
                    PositionSource::Internal => t_string!(i18n, position_internal),
                    PositionSource::External => t_string!(i18n, position_external),
                }
                .to_owned()
            });
        view! {
            <section class="card node-details">
                <dl>
                    {row(t!(i18n, nodes_id).into_any(), Some(format!("!{num:08x}")))}
                    {row(
                        t!(i18n, local_hardware).into_any(),
                        node.as_ref().and_then(|n| n.hw_model.as_deref().map(model)),
                    )}
                    {row(t!(i18n, local_role).into_any(), node.as_ref().and_then(|n| n.role.clone()))}
                    {row(t!(i18n, nodes_heard).into_any(), heard)}
                    {row(t!(i18n, nodes_hops).into_any(), hops)}
                    {row(t!(i18n, node_signal).into_any(), signal)}
                    {row(t!(i18n, nodes_battery).into_any(), battery)}
                    {row(
                        t!(i18n, node_channel_use).into_any(),
                        node.as_ref().and_then(|n| n.channel_utilization).map(|busy| format!("{busy:.1} %")),
                    )}
                    {row(t!(i18n, node_position).into_any(), position)}
                    {row(t!(i18n, node_precision).into_any(), precision)}
                    {row(t!(i18n, node_source).into_any(), source)}
                </dl>
            </section>
        }
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
    let buttons = move || {
        kinds
            .into_iter()
            .map(|kind| {
                view! {
                    <button
                        disabled=move || !connected() || waiting(kind)
                        on:click=move |_| live.ask(num, kind, channel.get_untracked())
                    >
                        <KindLabel kind />
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
            fallback=move || view! { <p class="card dim">{t!(i18n, meshcore_not_yet)}</p> }
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
                        disabled=move || !connected()
                        on:click=move |_| live.trace(num, channel.get_untracked())
                    >
                        {t!(i18n, ask_traceroute)}
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
