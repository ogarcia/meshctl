// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The map: the selected radio's nodes where they say they are. Leaflet draws
//! it, through the few calls `map.js` gives; what to draw, words and all, is
//! worked out here.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::html;
use leptos::prelude::*;
use meshctl_api::{Node, Peer, RadioId};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = open)]
    fn map_open(element: &web_sys::HtmlElement, pick: &JsValue, act: &JsValue);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = close)]
    fn map_close();
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = nodes)]
    fn map_nodes(json: &str);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = reset)]
    fn map_reset();
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = resize)]
    fn map_resize();
}

/// What the map is given to draw.
#[derive(Debug, Serialize)]
struct Drawing {
    labels: Labels,
    nodes: Vec<Marker>,
}

/// The words the map shows, in the interface's language.
#[derive(Debug, Serialize)]
struct Labels {
    page: String,
    chat: String,
    /// With `{n}` where the number of nodes goes.
    here: String,
    zoom: String,
}

/// A node as the map draws it.
#[derive(Debug, Serialize)]
struct Marker {
    num: u32,
    id: String,
    name: String,
    short: String,
    /// How long ago it was heard.
    seen: String,
    /// What its popup tells, label and value.
    facts: Vec<(String, String)>,
    lat: f64,
    lon: f64,
    /// `local`, `direct`, `near` or `far`, which the map colours.
    kind: &'static str,
    selected: bool,
}

/// What kind of node it is to the radio: itself, heard directly, a hop or
/// two away, or further.
pub fn kind(local: Option<u32>, num: u32, hops: Option<u32>, via_mqtt: bool) -> &'static str {
    if local == Some(num) {
        "local"
    } else if via_mqtt {
        "far"
    } else {
        match hops {
            Some(0) => "direct",
            Some(1..=2) => "near",
            _ => "far",
        }
    }
}

/// How many of the selected radio's nodes say where they are.
pub fn placed(live: Live) -> usize {
    live.nodes
        .with(|nodes| nodes.values().filter(|n| n.position.is_some()).count())
}

#[component]
pub fn MapView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let element = NodeRef::<html::Div>::new();

    // Opened once the element is there, closed when the page goes.
    Effect::new(move |opened: Option<bool>| {
        if opened == Some(true) {
            return true;
        }
        let Some(element) = element.get() else {
            return false;
        };
        let pick = Closure::<dyn Fn(Option<u32>)>::new(move |num| live.map_pick.set(num));
        let act = Closure::<dyn Fn(u32, String)>::new(move |num, what: String| {
            if what == "chat" {
                live.open_chat(Peer::Node { node: num });
            } else {
                live.open_node(num);
            }
        });
        map_open(&element, &pick.into_js_value(), &act.into_js_value());
        true
    });
    on_cleanup(map_close);

    // Another radio's nodes are somewhere else: the view goes to them.
    Effect::new(move |previous: Option<Option<RadioId>>| {
        let selected = live.selected.get();
        if previous.is_some_and(|previous| previous != selected) {
            map_reset();
        }
        selected
    });

    // Drawn again whenever the nodes, the pick, the time, the colours or the
    // language change.
    Effect::new(move |_| {
        if element.get().is_none() {
            return;
        }
        live.preferences.track();
        let now = live.now.get();
        let local = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        let picked = live.map_pick.get();
        let fact_labels = FactLabels {
            heard: t_string!(i18n, nodes_heard).to_owned(),
            hops: t_string!(i18n, nodes_hops).to_owned(),
            signal: t_string!(i18n, node_signal).to_owned(),
            battery: t_string!(i18n, nodes_battery).to_owned(),
            hardware: t_string!(i18n, local_hardware).to_owned(),
        };
        let mut markers: Vec<Marker> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter_map(|n| {
                    let position = n.position.as_ref()?;
                    Some(Marker {
                        num: n.num,
                        id: n.id.clone(),
                        name: n.long_name.clone().unwrap_or_else(|| n.id.clone()),
                        short: n.short_name.clone().unwrap_or_default(),
                        seen: n.last_heard.map(|at| ago(now, at)).unwrap_or_default(),
                        facts: facts(n, now, &fact_labels),
                        lat: position.latitude,
                        lon: position.longitude,
                        kind: kind(local, n.num, n.hops_away, n.via_mqtt),
                        selected: picked == Some(n.num),
                    })
                })
                .collect()
        });
        // Drawn in order: the radio's own node and the one picked last, on top.
        markers.sort_by_key(|m| (m.selected, m.kind == "local"));
        let drawing = Drawing {
            labels: Labels {
                page: t_string!(i18n, node_page).to_owned(),
                chat: t_string!(i18n, node_message).to_owned(),
                here: t_string!(i18n, map_here).to_owned(),
                zoom: t_string!(i18n, map_zoom).to_owned(),
            },
            nodes: markers,
        };
        if let Ok(json) = serde_json::to_string(&drawing) {
            map_nodes(&json);
        }
    });

    // The page's size changes with the window and with the bars.
    let resize = window_event_listener(leptos::ev::resize, move |_| map_resize());
    on_cleanup(move || resize.remove());

    view! {
        <div class="map-page">
            <div class="map" node_ref=element></div>
            <div class="map-legend">
                <span><i class="local"></i>{t!(i18n, map_local)}</span>
                <span><i class="direct"></i>{t!(i18n, map_direct)}</span>
                <span><i class="near"></i>{t!(i18n, map_near)}</span>
                <span><i class="far"></i>{t!(i18n, map_far)}</span>
            </div>
            <Show when=move || placed(live) == 0>
                <p class="map-empty card dim">{t!(i18n, map_empty)}</p>
            </Show>
        </div>
    }
}

/// The labels of a node's facts, in the interface's language.
struct FactLabels {
    heard: String,
    hops: String,
    signal: String,
    battery: String,
    hardware: String,
}

/// What a node's popup tells: whatever is known of these.
fn facts(node: &Node, now: u32, labels: &FactLabels) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    if let Some(heard) = node.last_heard {
        facts.push((labels.heard.clone(), ago(now, heard)));
    }
    let hops = if node.via_mqtt {
        Some("MQTT".to_owned())
    } else {
        node.hops_away.map(|h| h.to_string())
    };
    if let Some(hops) = hops {
        facts.push((labels.hops.clone(), hops));
    }
    // Heard through others, the SNR is the last relay's, not this node's.
    if let Some(snr) = node
        .snr
        .filter(|_| node.hops_away == Some(0) && !node.via_mqtt)
    {
        let signal = match node.rssi {
            Some(rssi) => format!("{snr:.1} dB · {rssi} dBm"),
            None => format!("{snr:.1} dB"),
        };
        facts.push((labels.signal.clone(), signal));
    }
    let battery = match (node.battery_level, node.voltage) {
        (Some(level), _) if level > 100 => Some("⚡".to_owned()),
        (Some(level), Some(voltage)) => Some(format!("{level} % · {voltage:.2} V")),
        (Some(level), None) => Some(format!("{level} %")),
        (None, Some(voltage)) => Some(format!("{voltage:.2} V")),
        (None, None) => None,
    };
    if let Some(battery) = battery {
        facts.push((labels.battery.clone(), battery));
    }
    if let Some(hardware) = &node.hw_model {
        facts.push((labels.hardware.clone(), hardware.clone()));
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_are_coloured_by_how_far_they_are() {
        assert_eq!(kind(Some(1), 1, Some(0), false), "local");
        assert_eq!(kind(Some(1), 2, Some(0), false), "direct");
        assert_eq!(kind(Some(1), 2, Some(2), false), "near");
        assert_eq!(kind(Some(1), 2, Some(3), false), "far");
        assert_eq!(kind(Some(1), 2, Some(0), true), "far");
        assert_eq!(kind(None, 2, None, false), "far");
    }

    #[test]
    fn a_popup_tells_what_is_known() {
        let node = Node {
            num: 1,
            id: "!00000001".into(),
            long_name: Some("Uno".into()),
            short_name: None,
            hw_model: Some("HELTEC_V3".into()),
            role: None,
            position: None,
            last_heard: Some(940),
            snr: Some(6.5),
            rssi: Some(-90),
            hops_away: Some(2),
            via_mqtt: false,
            favorite: false,
            battery_level: Some(80),
            voltage: None,
        };
        let labels = FactLabels {
            heard: "heard".into(),
            hops: "hops".into(),
            signal: "signal".into(),
            battery: "battery".into(),
            hardware: "hardware".into(),
        };
        let told: Vec<String> = facts(&node, 1000, &labels)
            .into_iter()
            .map(|(label, value)| format!("{label}={value}"))
            .collect();
        // Two hops away: no SNR of its own.
        assert_eq!(
            told,
            [
                "heard=1 min",
                "hops=2",
                "battery=80 %",
                "hardware=HELTEC_V3"
            ]
        );
    }
}
