// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio hears, packet by packet, as it hears it: the
//! newest on top, older ones a page at a time below.

use crate::app::Live;
use crate::chat::{node_name, short_name, title};
use crate::daemon;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{BROADCAST, Packet, Peer};
use serde_json::Value;

/// The most packets kept in the page: older ones go as new ones come.
pub const KEPT: usize = 1000;

#[component]
pub fn PacketsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let busy = RwSignal::new(false);
    let older = move |_| {
        let (Some(radio), Some(last)) = (
            live.selected.get_untracked(),
            live.packets.with_untracked(|p| p.last().map(|p| p.id)),
        ) else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            if let Ok(page) = daemon::packets(radio, Some(last)).await
                && live.selected.get_untracked() == Some(radio)
            {
                live.packets.update(|packets| packets.extend(page));
            }
            busy.set(false);
        });
    };

    view! {
        <section class="card packets">
            <Show
                when=move || live.packets.with(|p| !p.is_empty())
                fallback=move || view! { <p class="dim">{t!(i18n, packets_empty)}</p> }
            >
                <ol class="packet-list">
                    <For each=move || live.packets.get() key=|p| p.id let:packet>
                        <PacketRow packet />
                    </For>
                </ol>
                <button class="older" disabled=move || busy.get() on:click=older>
                    {t!(i18n, packets_older)}
                </button>
            </Show>
        </section>
    }
}

#[component]
fn PacketRow(packet: Packet) -> impl IntoView {
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
        <li class="packet" class:open=move || open.get()>
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
