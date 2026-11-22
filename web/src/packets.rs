// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio hears, packet by packet, as it hears it: the
//! newest on top, each one new glowing once as it comes, older ones read as
//! the list is scrolled down. Filters say which show; it can be paused, and
//! it holds still while it is read away from its top, what comes waiting
//! above until it goes back.

use crate::app::Live;
use crate::chat::{node_name, short_name, title};
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use crate::preferences::{Choice, pills};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{BROADCAST, Packet, PacketFilter, PacketKind, PacketReach, Peer};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

const PAUSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/player-pause.svg");
const PLAY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/player-play.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-up.svg");

/// The most packets kept in the page: older ones go as new ones come.
pub const KEPT: usize = 1000;

/// How far from the top the list is being read, and held still.
const AWAY: i32 = 24;

/// How near its end older packets are read.
const NEAR_END: i32 = 600;

/// The page's scroller, which the list scrolls in.
fn scroller() -> Option<web_sys::Element> {
    document().query_selector("main").ok().flatten()
}

#[component]
pub fn PacketsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();

    // Opened at its top: what came while it was not shown goes on, and none
    // of what there is now is new.
    live.packets_away.set(false);
    if !live.packets_paused.get_untracked() {
        live.release_packets();
    }
    live.packets_mark.set(
        live.packets
            .with_untracked(|p| p.first().map(|p| p.id))
            .unwrap_or_else(|| live.packets_mark.get_untracked()),
    );

    // Older ones, a page at a time, while there are.
    let busy = RwSignal::new(false);
    let older = move || {
        if busy.get_untracked() || live.packets_end.get_untracked() {
            return;
        }
        let (Some(radio), Some(last)) = (
            live.selected.get_untracked(),
            live.packets.with_untracked(|p| p.last().map(|p| p.id)),
        ) else {
            return;
        };
        let filter = live.packets_filter();
        busy.set(true);
        spawn_local(async move {
            if let Ok(page) = daemon::packets(radio, Some(last), &filter).await
                && live.selected.get_untracked() == Some(radio)
                && live.packets_filter() == filter
            {
                if page.len() < daemon::PACKETS_PAGE {
                    live.packets_end.set(true);
                }
                live.packets.update(|packets| packets.extend(page));
            }
            busy.set(false);
        });
    };

    // Back at the top, what came goes on, unless paused.
    let to_top = move || {
        if let Some(main) = scroller() {
            main.set_scroll_top(0);
        }
        live.packets_away.set(false);
        live.release_packets();
    };

    // As it is scrolled: away from the top or back, and near the end. The
    // listener goes with the view.
    let on_scroll = move || {
        let Some(main) = scroller() else { return };
        let top = main.scroll_top();
        let away = top > AWAY;
        if away != live.packets_away.get_untracked() {
            live.packets_away.set(away);
            if !away && !live.packets_paused.get_untracked() {
                live.release_packets();
            }
        }
        if main.scroll_height() - top - main.client_height() < NEAR_END {
            older();
        }
    };
    let listener = StoredValue::new_local(None::<(web_sys::Element, Closure<dyn FnMut()>)>);
    if let Some(main) = scroller() {
        let call = Closure::<dyn FnMut()>::new(on_scroll);
        if main
            .add_event_listener_with_callback("scroll", call.as_ref().unchecked_ref())
            .is_ok()
        {
            listener.set_value(Some((main, call)));
        }
    }
    on_cleanup(move || {
        listener.update_value(|l| {
            if let Some((main, call)) = l.take() {
                let _ = main
                    .remove_event_listener_with_callback("scroll", call.as_ref().unchecked_ref());
            }
        });
    });
    // Short of filling the page, older ones too, until it does or there are
    // no more.
    Effect::new(move |_| {
        live.packets.with(Vec::len);
        live.packets_end.track();
        request_animation_frame(on_scroll);
    });

    // Filtered anew: read again from the top. Only as the filters change, not
    // as they are set to what they were, as a field left does.
    Effect::new(move |last: Option<(PacketFilter, bool)>| {
        let now = (live.packet_filter.get(), live.packets_hide_own.get());
        if last.is_some_and(|last| last != now) {
            if let Some(main) = scroller() {
                main.set_scroll_top(0);
            }
            live.packets_away.set(false);
            spawn_local(async move {
                let _ = live.reload_packets().await;
            });
        }
        now
    });

    let filtered = move || {
        live.packet_filter.with(|f| *f != PacketFilter::default()) || live.packets_hide_own.get()
    };
    let held = move || live.packets_held.with(Vec::len);

    view! {
        <div class="live">
            <PacketTools />
            <section class="packets">
                <Show when=move || held() != 0>
                    <button class="new-packets" on:click=move |_| to_top()>
                        <span class="icon" inner_html=UP></span>
                        {move || t!(i18n, packets_new, count = held())}
                    </button>
                </Show>
                <Show
                    when=move || live.packets.with(|p| !p.is_empty())
                    fallback=move || {
                        view! {
                            <p class="dim nothing">
                                {move || {
                                    if filtered() {
                                        t!(i18n, packets_none).into_any()
                                    } else {
                                        t!(i18n, packets_empty).into_any()
                                    }
                                }}
                            </p>
                        }
                    }
                >
                    <ol class="packet-list">
                        <For each=move || live.packets.get() key=|p| p.id let:packet>
                            {
                                let fresh = packet.id > live.packets_mark.get_untracked();
                                view! { <PacketRow packet fresh /> }
                            }
                        </For>
                    </ol>
                    <Show when=move || live.packets_end.get()>
                        <p class="dim end">{t!(i18n, packets_end)}</p>
                    </Show>
                </Show>
            </section>
        </div>
    }
}

/// The filters over the list, and its pause.
#[component]
fn PacketTools() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = live.packet_filter;

    // A kind by its name, and its name in the query of the list.
    let kind_label = move |kind: PacketKind| match kind {
        PacketKind::Text => t_string!(i18n, packets_kind_text),
        PacketKind::Position => t_string!(i18n, packets_kind_position),
        PacketKind::NodeInfo => t_string!(i18n, packets_kind_node_info),
        PacketKind::Routing => t_string!(i18n, packets_kind_routing),
        PacketKind::Telemetry => t_string!(i18n, packets_kind_telemetry),
        PacketKind::Traceroute => t_string!(i18n, packets_kind_traceroute),
        PacketKind::Neighbors => t_string!(i18n, packets_kind_neighbors),
        PacketKind::Encrypted => t_string!(i18n, packets_kind_encrypted),
        PacketKind::Other => t_string!(i18n, packets_kind_other),
    };
    let kind_value = |kind: PacketKind| {
        serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let pick_kind = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        let kind = PacketKind::ALL
            .into_iter()
            .find(|k| kind_value(*k) == value);
        filter.update(|f| f.kind = kind);
    };

    // The channels the radio has in use, by their slots.
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect::<Vec<_>>()
        })
    };
    let pick_channel = move |ev: leptos::ev::Event| {
        let channel = event_target_value(&ev).parse().ok();
        filter.update(|f| f.channel = channel);
    };

    // How they came, as the nodes list says it.
    let reaches: Vec<Choice> = [
        None,
        Some(PacketReach::Direct),
        Some(PacketReach::Mesh),
        Some(PacketReach::Mqtt),
    ]
    .into_iter()
    .map(|reach| {
        let label = move || match reach {
            None => t_string!(i18n, nodes_all).to_owned(),
            Some(PacketReach::Direct) => t_string!(i18n, nodes_direct).to_owned(),
            Some(PacketReach::Mesh) => t_string!(i18n, nodes_mesh).to_owned(),
            Some(PacketReach::Mqtt) => "MQTT".to_owned(),
        };
        (
            Signal::derive(move || filter.with(|f| f.reach) == reach),
            Box::new(move || filter.update(|f| f.reach = reach)) as Box<dyn Fn() + Send + Sync>,
            Signal::derive(label),
        )
    })
    .collect();

    // A node by its name as the list offers it, or by its id written out.
    let node_label = move |num: u32| format!("{} (!{num:08x})", node_name(live, num));
    let pick_node = move |ev: leptos::ev::Event| {
        let text = event_target_value(&ev);
        let text = text.trim();
        let node = if text.is_empty() {
            None
        } else if let Some(hex) = text.strip_prefix('!')
            && let Ok(num) = u32::from_str_radix(hex, 16)
        {
            Some(num)
        } else {
            live.nodes
                .with_untracked(|nodes| nodes.keys().copied().find(|&num| node_label(num) == text))
        };
        // Only once it names one, or is cleared: not as it is written.
        if node.is_some() || text.is_empty() {
            filter.update(|f| f.node = node);
        }
    };

    view! {
        <div class="packet-tools">
            <select
                class="kind"
                aria-label=move || t_string!(i18n, packets_kind)
                prop:value=move || filter.with(|f| f.kind.map(kind_value).unwrap_or_default())
                on:change=pick_kind
            >
                <option value="">{t!(i18n, packets_kind_all)}</option>
                {PacketKind::ALL
                    .into_iter()
                    .map(|kind| {
                        view! { <option value=kind_value(kind)>{move || kind_label(kind)}</option> }
                    })
                    .collect_view()}
            </select>
            <select
                class="channel"
                aria-label=move || t_string!(i18n, packets_channel)
                prop:value=move || filter.with(|f| f.channel.map(|c| c.to_string()).unwrap_or_default())
                on:change=pick_channel
            >
                <option value="">{t!(i18n, packets_channel_all)}</option>
                <For each=channels key=|slot| *slot let:slot>
                    <option value=slot.to_string()>
                        {move || format!("# {}", title(live, Peer::Channel { channel: slot }))}
                    </option>
                </For>
            </select>
            {pills(reaches)}
            <input
                type="search"
                class="node-filter"
                list="packet-nodes"
                placeholder=move || t_string!(i18n, packets_node)
                prop:value=move || filter.with(|f| f.node).map(node_label).unwrap_or_default()
                on:change=pick_node
                on:search=pick_node
            />
            <datalist id="packet-nodes">
                <For each=move || live.nodes.with(|n| n.keys().copied().collect::<Vec<_>>()) key=|num| *num let:num>
                    <option value=move || node_label(num)></option>
                </For>
            </datalist>
            <button
                type="button"
                class="choice"
                class:on=move || live.packets_hide_own.get()
                aria-pressed=move || live.packets_hide_own.get().to_string()
                on:click=move |_| live.packets_hide_own.update(|h| *h = !*h)
            >
                {t!(i18n, packets_hide_own)}
            </button>
        </div>
    }
}

/// The live view's pause, in its heading: going on, what came while it was
/// paused goes on top, unless it is being read further down.
#[component]
pub fn PauseButton() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let paused = live.packets_paused;
    let pause = move |_| {
        let now = !paused.get_untracked();
        paused.set(now);
        if !now && !live.packets_away.get_untracked() {
            live.release_packets();
        }
    };
    let label = move || {
        if paused.get() {
            t_string!(i18n, packets_resume)
        } else {
            t_string!(i18n, packets_pause)
        }
    };
    view! {
        <button
            type="button"
            class="with-icon pause"
            class:on=move || paused.get()
            aria-pressed=move || paused.get().to_string()
            title=label
            on:click=pause
        >
            <span class="icon" inner_html=move || if paused.get() { PLAY } else { PAUSE }></span>
            <span class="label">{label}</span>
        </button>
    }
}

#[component]
fn PacketRow(packet: Packet, fresh: bool) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let from = packet.from;
    let to = packet.to;
    let who = move |num: u32| {
        let short = short_name(live, num);
        let long = node_name(live, num);
        if short.is_empty() || long.starts_with('!') {
            long
        } else {
            format!("{short} · {long}")
        }
    };
    let reception = {
        let mut parts = Vec::new();
        if packet.via_mqtt {
            parts.push("MQTT".to_owned());
        } else if let Some(hops) = packet.hops_away {
            parts.push(format!("{hops} ⇢"));
        }
        if let Some(snr) = packet.snr {
            parts.push(format!("{snr:.1} dB"));
        }
        if let Some(rssi) = packet.rssi {
            parts.push(format!("{rssi} dBm"));
        }
        parts.join(" · ")
    };
    let port = packet.port.clone().map(|p| short_port(&p));
    let encrypted = packet.port.is_none();
    let summary = summary(&packet);
    let detail = packet
        .payload
        .as_ref()
        .and_then(|payload| serde_json::to_string_pretty(payload).ok());
    let has_detail = detail.is_some();

    view! {
        <li class="packet" class:open=move || open.get() class:arrived=fresh>
            <button class="packet-line" on:click=move |_| open.update(|o| *o = !*o) disabled=!has_detail>
                <span class="time mono">{clock(packet.at)}</span>
                <span class="port">
                    {match port {
                        Some(port) => port.into_any(),
                        None => t!(i18n, packets_encrypted).into_any(),
                    }}
                </span>
                <span class="route">
                    <span class="from">{move || who(from)}</span>
                    " → "
                    <span class="to">
                        {move || {
                            if to == BROADCAST {
                                title(live, Peer::Channel { channel: packet.channel })
                            } else {
                                who(to)
                            }
                        }}
                    </span>
                </span>
                <span class="summary" class:dim=encrypted>{summary}</span>
                <span class="reception mono dim">{reception}</span>
            </button>
            <Show when=move || open.get()>
                <pre class="payload mono">{detail.clone()}</pre>
            </Show>
        </li>
    }
}

/// A port without its `_APP`, as people say it.
fn short_port(port: &str) -> String {
    port.strip_suffix("_APP").unwrap_or(port).replace('_', " ")
}

/// The time of day, in the browser's own zone.
fn clock(at: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let date = js_sys::Date::new_0();
    date.set_time(at as f64 * 1000.0);
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// What a packet says, in a line.
pub fn summary(packet: &Packet) -> String {
    let Some(payload) = &packet.payload else {
        return String::new();
    };
    let port = packet.port.as_deref().unwrap_or_default();
    let line = match port {
        _ if payload.is_string() => payload.as_str().map(str::to_owned),
        "POSITION_APP" => position(payload),
        "NODEINFO_APP" => {
            let name = payload["long_name"].as_str().unwrap_or_default();
            let model = payload["hw_model"].as_str().unwrap_or_default();
            Some(format!("{name} · {model}"))
        }
        "TELEMETRY_APP" => telemetry(payload),
        "ROUTING_APP" => payload["error_reason"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| Some("NONE".into())),
        "TRACEROUTE_APP" => {
            let relays = payload["route"].as_array().map_or(0, Vec::len);
            Some(format!("{relays} ⇢"))
        }
        "NEIGHBORINFO_APP" => {
            let neighbors = payload["neighbors"].as_array().map_or(0, Vec::len);
            Some(format!("{neighbors} ↔"))
        }
        _ => None,
    };
    line.unwrap_or_else(|| {
        let mut json = payload.to_string();
        if json.len() > 120 {
            let cut = (0..=117)
                .rev()
                .find(|&i| json.is_char_boundary(i))
                .unwrap_or(0);
            json.truncate(cut);
            json.push('…');
        }
        json
    })
}

fn position(payload: &Value) -> Option<String> {
    let degrees = |key: &str| payload[key].as_f64().map(|v| v * 1e-7);
    let (latitude, longitude) = (degrees("latitude_i")?, degrees("longitude_i")?);
    let altitude = payload["altitude"]
        .as_i64()
        .map_or_else(String::new, |a| format!(" · {a} m"));
    Some(format!("{latitude:.5}, {longitude:.5}{altitude}"))
}

fn telemetry(payload: &Value) -> Option<String> {
    let parts: Vec<String> = [
        ("device_metrics", "battery_level", "%"),
        ("device_metrics", "voltage", "V"),
        ("device_metrics", "channel_utilization", "% ch"),
        ("device_metrics", "air_util_tx", "% tx"),
        ("environment_metrics", "temperature", "°C"),
        ("environment_metrics", "relative_humidity", "% RH"),
        ("environment_metrics", "barometric_pressure", "hPa"),
        ("power_metrics", "ch1_voltage", "V"),
        ("local_stats", "num_online_nodes", "online"),
    ]
    .iter()
    .filter_map(|(group, key, unit)| {
        let value = payload[group][key].as_f64()?;
        Some(if value.fract() == 0.0 {
            format!("{value:.0} {unit}")
        } else {
            format!("{value:.1} {unit}")
        })
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(port: &str, payload: Value) -> Packet {
        Packet {
            id: 1,
            packet_id: 1,
            from: 1,
            to: BROADCAST,
            channel: 0,
            port: Some(port.into()),
            payload: Some(payload),
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: 0,
            snr: None,
            rssi: None,
            hops_away: None,
            hop_limit: 0,
            hop_start: 0,
            relay_node: None,
            via_mqtt: false,
        }
    }

    #[test]
    fn packets_read_in_a_line() {
        assert_eq!(summary(&packet("TEXT_MESSAGE_APP", "hola".into())), "hola");
        let position = serde_json::json!({"latitude_i": 433_000_000, "longitude_i": -84_000_000});
        assert_eq!(
            summary(&packet("POSITION_APP", position)),
            "43.30000, -8.40000"
        );
        let telemetry =
            serde_json::json!({"device_metrics": {"battery_level": 101, "voltage": 4.2}});
        assert_eq!(
            summary(&packet("TELEMETRY_APP", telemetry)),
            "101 % · 4.2 V"
        );
        assert_eq!(short_port("TEXT_MESSAGE_APP"), "TEXT MESSAGE");
    }
}
