// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The map: the selected radio's nodes where they say they are. Leaflet draws
//! it, through the few calls `map.js` gives; what to draw, words and all, is
//! worked out here.

use crate::app::Live;
use crate::chat::title;
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::{ago, model, power, radius};
use crate::traces::quality;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    Hop, MapView, Node, Peer, PositionSource, RadioId, RadioKind, Traceroute, TracerouteId,
    TracerouteStatus,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = open)]
    fn map_open(element: &web_sys::HtmlElement, pick: &JsValue, act: &JsValue, moved: &JsValue);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = close)]
    fn map_close();
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = nodes)]
    fn map_nodes(json: &str);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = route)]
    fn map_route(json: Option<String>);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = stay)]
    fn map_stay();
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = reset)]
    fn map_reset();
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = view)]
    fn map_view(json: Option<String>);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = resize)]
    fn map_resize();
}

/// What the map is given to draw.
#[derive(Debug, Serialize)]
struct Drawing {
    labels: Labels,
    nodes: Vec<Marker>,
    /// Whether the nodes are all of the selected radio's, not some of them
    /// while they are read: only then is the map placed by them.
    ready: bool,
}

/// The words the map shows, in the interface's language.
#[derive(Debug, Serialize)]
struct Labels {
    page: String,
    chat: String,
    /// With `{n}` where the number of nodes goes.
    here: String,
    zoom: String,
    /// The button that takes in every node.
    all: String,
    /// The route button, by what it does: named as [`Marker::route`] says.
    show: String,
    hide: String,
    trace: String,
    tracing: String,
}

/// A route as the map draws it.
#[derive(Debug, Serialize)]
struct RouteDrawing {
    /// The traceroute it is, which the map goes to the first time it is shown.
    id: TracerouteId,
    towards: Vec<Segment>,
    back: Vec<Segment>,
    /// Where the relays on the way are.
    relays: Vec<(f64, f64)>,
    /// Every node on it, both ways: the only ones the map shows while it is.
    nodes: Vec<u32>,
}

/// A stretch of a route between two nodes the map can place.
#[derive(Debug, Serialize)]
struct Segment {
    from: (f64, f64),
    to: (f64, f64),
    /// How well the last hop of it was heard, as [`quality`] says.
    quality: &'static str,
    /// Whether there are nodes between its ends that could not be placed.
    gap: bool,
}

/// The stretches of a route that can be drawn: between each pair of hops
/// whose nodes say where they are, across those that do not.
fn segments(hops: &[Hop], at: &impl Fn(u32) -> Option<(f64, f64)>) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut last: Option<(f64, f64)> = None;
    let mut gap = false;
    for hop in hops {
        match hop.node.and_then(at) {
            Some(here) => {
                if let Some(from) = last {
                    segments.push(Segment {
                        from,
                        to: here,
                        quality: if gap { "unknown" } else { quality(hop.snr) },
                        gap,
                    });
                }
                last = Some(here);
                gap = false;
            }
            None => gap = true,
        }
    }
    segments
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
    /// How it is heard, in a line: its signal when directly, its hops when
    /// through others, MQTT when over it.
    heard: String,
    /// What its popup tells.
    facts: Vec<Fact>,
    lat: f64,
    lon: f64,
    /// `local`, `direct`, `near` (1–2 hops), `mid` (3–4) or `far`, which the
    /// map colours.
    kind: &'static str,
    /// What it is there for, which the map marks with an icon, as [`role`] says.
    role: Option<&'static str>,
    selected: bool,
    /// For the node picked, what its route button does: `show`, `hide`,
    /// `trace` or `tracing`; none when routes cannot be traced.
    route: Option<&'static str>,
}

/// The node's traceroutes the map cares for: the newest, and the newest that
/// was answered, which may be the same.
fn traces_of(live: Live, node: u32) -> (Option<Traceroute>, Option<Traceroute>) {
    live.traceroutes.with(|traces| {
        let mut theirs = traces.iter().filter(|t| t.target == node);
        let newest = theirs.next().cloned();
        let done = std::iter::once(newest.clone())
            .flatten()
            .chain(theirs.cloned())
            .find(|t| t.status == TracerouteStatus::Done);
        (newest, done)
    })
}

/// What a node is there for, when the map marks it with an icon: `router`
/// for those that carry the mesh (routers and repeaters, Meshtastic's and
/// MeshCore's), `base` for a client at a fixed base station.
pub fn role(role: Option<&str>) -> Option<&'static str> {
    match role? {
        "ROUTER" | "ROUTER_LATE" | "ROUTER_CLIENT" | "REPEATER" => Some("router"),
        "CLIENT_BASE" => Some("base"),
        _ => None,
    }
}

/// What kind of node it is to the radio: itself, heard directly, one or two
/// hops away, three or four, or further (or over MQTT, or nobody knows).
pub fn kind(local: Option<u32>, num: u32, hops: Option<u32>, via_mqtt: bool) -> &'static str {
    if local == Some(num) {
        "local"
    } else if via_mqtt {
        "far"
    } else {
        match hops {
            Some(0) => "direct",
            Some(1..=2) => "near",
            Some(3..=4) => "mid",
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
    // The radio whose nodes the map was placed for, which is not yet the
    // selected one while it changes.
    let on_map = StoredValue::new(None::<RadioId>);

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
            match what.as_str() {
                "chat" => live.open_chat(Peer::Node { node: num }),
                "unroute" => live.map_route.set(None),
                "route" => {
                    if live.map_route.get_untracked() == Some(num) {
                        live.map_route.set(None);
                    } else {
                        // With nothing traced yet, it is traced now, and shown
                        // as the answer comes.
                        if traces_of(live, num).1.is_none() {
                            live.trace(num, 0);
                        }
                        live.map_route.set(Some(num));
                    }
                }
                _ => live.open_node(num),
            }
        });
        // Where the person leaves the map, kept for the radio it shows.
        let moved = Closure::<dyn Fn(f64, f64, f64)>::new(move |latitude, longitude, zoom| {
            let Some(radio) = on_map.get_value() else {
                return;
            };
            let view = MapView {
                latitude,
                longitude,
                zoom,
            };
            spawn_local(async move {
                let _ = daemon::save_map_view(radio, &view).await;
            });
        });
        map_open(
            &element,
            &pick.into_js_value(),
            &act.into_js_value(),
            &moved.into_js_value(),
        );
        true
    });
    on_cleanup(map_close);

    // Another radio's nodes are somewhere else: the view goes to where it was
    // left for that one, or to its nodes. Once the map is there, and again
    // as the radio changes.
    Effect::new(move |previous: Option<Option<RadioId>>| {
        let selected = live.selected.get();
        // Not before the map is there; then, as if for the first time.
        element.get()?;
        if previous.is_some_and(|previous| previous != selected) {
            // Where the last radio was left is kept for it first.
            map_reset();
        }
        on_map.set_value(selected);
        if let Some(radio) = selected {
            spawn_local(async move {
                let view = daemon::map_view(radio).await.ok().flatten();
                if live.selected.get_untracked() == Some(radio) {
                    map_view(view.and_then(|v| serde_json::to_string(&v).ok()));
                }
            });
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
        let ready = live.selected.get().is_some() && live.loaded.get() == live.selected.get();
        let shown_route = live.map_route.get();
        let traceable = live.selected_kind() != Some(RadioKind::Meshcore);
        let route_button = |num: u32| -> Option<&'static str> {
            if picked != Some(num) || !traceable || local == Some(num) {
                return None;
            }
            let (newest, done) = traces_of(live, num);
            Some(
                if newest.is_some_and(|t| t.status == TracerouteStatus::Pending) {
                    "tracing"
                } else if shown_route == Some(num) && done.is_some() {
                    "hide"
                } else if done.is_some() {
                    "show"
                } else {
                    "trace"
                },
            )
        };
        let hop_words = HopWords {
            direct: t_string!(i18n, map_direct).to_owned(),
            one: t_string!(i18n, map_one_hop).to_owned(),
            many: t_string!(i18n, map_hops).to_owned(),
        };
        let fact_labels = FactLabels {
            heard: t_string!(i18n, nodes_heard).to_owned(),
            hops: t_string!(i18n, nodes_hops).to_owned(),
            signal: t_string!(i18n, node_signal).to_owned(),
            battery: t_string!(i18n, nodes_battery).to_owned(),
            external: t_string!(i18n, power_external).to_owned(),
            direct: t_string!(i18n, map_direct).to_owned(),
            altitude: t_string!(i18n, node_altitude).to_owned(),
            precision: t_string!(i18n, node_precision).to_owned(),
            position: t_string!(i18n, node_position).to_owned(),
            manual: t_string!(i18n, position_manual).to_owned(),
            internal: t_string!(i18n, position_internal).to_owned(),
            external_gps: t_string!(i18n, position_external).to_owned(),
            role: t_string!(i18n, local_role).to_owned(),
            channel: t_string!(i18n, node_channel_use).to_owned(),
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
                        heard: heard(n, &hop_words),
                        facts: facts(n, now, &fact_labels),
                        lat: position.latitude,
                        lon: position.longitude,
                        kind: kind(local, n.num, n.hops_away, n.via_mqtt),
                        role: role(n.role.as_deref()),
                        selected: picked == Some(n.num),
                        route: route_button(n.num),
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
                all: t_string!(i18n, map_all).to_owned(),
                show: t_string!(i18n, map_route_show).to_owned(),
                hide: t_string!(i18n, map_route_hide).to_owned(),
                trace: t_string!(i18n, map_route_trace).to_owned(),
                tracing: t_string!(i18n, map_route_tracing).to_owned(),
            },
            nodes: markers,
            ready,
        };
        if let Ok(json) = serde_json::to_string(&drawing) {
            map_nodes(&json);
        }
    });

    // A route shown stays until its notice is closed or another node is
    // picked: letting the node go, with a click anywhere on the map, as when
    // dragging it, leaves it.
    Effect::new(move |previous: Option<Option<u32>>| {
        let picked = live.map_pick.get();
        if previous.is_some_and(|previous| previous != picked) {
            let shown = live.map_route.get_untracked();
            if shown.is_some() && picked.is_some() && shown != picked {
                // Not closed but left for another node: the map stays put.
                map_stay();
                live.map_route.set(None);
            }
        }
        picked
    });

    // The route shown, drawn again as its nodes or its traces change.
    Effect::new(move |_| {
        if element.get().is_none() {
            return;
        }
        live.preferences.track();
        let drawing = live.map_route.get().and_then(|num| {
            let traceroute = traces_of(live, num).1?;
            let at = |node: u32| {
                live.nodes.with(|nodes| {
                    let position = nodes.get(&node)?.position.as_ref()?;
                    Some((position.latitude, position.longitude))
                })
            };
            let relays = traceroute
                .towards
                .iter()
                .chain(&traceroute.back)
                .filter_map(|hop| hop.node)
                .filter(|&node| {
                    node != num && Some(node) != traceroute.towards.first().and_then(|h| h.node)
                })
                .filter_map(at)
                .collect();
            let mut on_it: Vec<u32> = traceroute
                .towards
                .iter()
                .chain(&traceroute.back)
                .filter_map(|hop| hop.node)
                .chain([num])
                .collect();
            on_it.sort_unstable();
            on_it.dedup();
            Some(RouteDrawing {
                id: traceroute.id,
                towards: segments(&traceroute.towards, &at),
                back: segments(&traceroute.back, &at),
                relays,
                nodes: on_it,
            })
        });
        map_route(drawing.and_then(|d| serde_json::to_string(&d).ok()));
    });

    // Escape takes the route away.
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            live.map_route.set(None);
        }
    });
    on_cleanup(move || escape.remove());

    let folded = move || live.preferences.with(|p| p.map_legend_folded);

    // The page's size changes with the window and with the bars.
    let resize = window_event_listener(leptos::ev::resize, move |_| map_resize());
    on_cleanup(move || resize.remove());

    view! {
        <div class="map-page">
            <div class="map" node_ref=element></div>
            <div class="map-legend" class:folded=folded>
                // Folds the legend away into itself, for more map, and back.
                <button
                    class="map-legend-toggle"
                    aria-expanded=move || (!folded()).to_string()
                    aria-label=move || {
                        if folded() {
                            t_string!(i18n, map_legend_show)
                        } else {
                            t_string!(i18n, map_legend_hide)
                        }
                    }
                    title=move || {
                        if folded() {
                            t_string!(i18n, map_legend_show)
                        } else {
                            t_string!(i18n, map_legend_hide)
                        }
                    }
                    on:click=move |_| {
                        let mut preferences = live.preferences.get_untracked();
                        preferences.map_legend_folded = !preferences.map_legend_folded;
                        live.prefer(preferences);
                    }
                >
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M15 6l-6 6l6 6"></path>
                    </svg>
                </button>
                <div class="map-legend-items">
                    <span><i class="local"></i>{t!(i18n, map_local)}</span>
                    <span><i class="direct"></i>{t!(i18n, map_direct)}</span>
                    <span><i class="near"></i>{t!(i18n, map_near)}</span>
                    <span><i class="mid"></i>{t!(i18n, map_mid)}</span>
                    <span><i class="far"></i>{t!(i18n, map_far)}</span>
                    <span><b class="icon router"></b>{t!(i18n, map_router)}</span>
                    <span><b class="icon base"></b>{t!(i18n, map_base)}</span>
                </div>
            </div>
            <Show when=move || placed(live) == 0>
                <p class="map-empty card dim">{t!(i18n, map_empty)}</p>
            </Show>
            <RouteNotice />
        </div>
    }
}

/// While a route is shown, what it is, and the way to take it away.
#[component]
fn RouteNotice() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        let num = live.map_route.get()?;
        let name = title(live, Peer::Node { node: num });
        let (newest, done) = traces_of(live, num);
        let what = match (&newest, &done) {
            (Some(t), _) if t.status == TracerouteStatus::Pending => {
                t!(i18n, map_route_tracing_to, name = name.clone()).into_any()
            }
            (_, Some(done)) => {
                let at = u32::try_from(done.at).unwrap_or_default();
                t!(
                    i18n,
                    map_route_to,
                    name = name.clone(),
                    ago = move || ago(live.now.get(), at)
                )
                .into_any()
            }
            _ => t!(i18n, map_route_none, name = name.clone()).into_any(),
        };
        Some(view! {
            <div class="map-route-notice" role="status">
                <span>{what}</span>
                <span class="route-key" aria-hidden="true">
                    <i></i>{t!(i18n, map_route_there)}
                    <i class="back"></i>{t!(i18n, map_route_back)}
                </span>
                <button
                    aria-label=move || t_string!(i18n, map_route_hide)
                    title=move || t_string!(i18n, map_route_hide)
                    on:click=move |_| live.map_route.set(None)
                    inner_html=CLOSE
                ></button>
            </div>
        })
    }
}

/// How hops are said, in the interface's language.
struct HopWords {
    /// Heard directly, when there is no signal to tell.
    direct: String,
    one: String,
    /// With `{n}` where the number of hops goes.
    many: String,
}

/// How a node is heard, in a line: the signal it was heard at when directly,
/// which says more than "no hops"; its hops when through others.
fn heard(node: &Node, words: &HopWords) -> String {
    if node.via_mqtt {
        return "MQTT".into();
    }
    match node.hops_away {
        Some(0) => match (node.snr, node.rssi) {
            (Some(snr), Some(rssi)) => format!("SNR {snr:.1} dB · RSSI {rssi} dBm"),
            (Some(snr), None) => format!("SNR {snr:.1} dB"),
            _ => words.direct.clone(),
        },
        Some(1) => words.one.clone(),
        Some(hops) => words.many.replace("{n}", &hops.to_string()),
        None => String::new(),
    }
}

/// The labels of a node's facts, in the interface's language.
struct FactLabels {
    heard: String,
    hops: String,
    signal: String,
    battery: String,
    /// The power, for a node plugged in.
    external: String,
    /// The hops, for a node heard directly.
    direct: String,
    altitude: String,
    precision: String,
    /// How a node came by its position, and the three ways it can.
    position: String,
    manual: String,
    internal: String,
    external_gps: String,
    role: String,
    /// How busy it hears the channel.
    channel: String,
    hardware: String,
}

/// What a node's popup tells: whatever is known of these.
fn facts(node: &Node, now: u32, labels: &FactLabels) -> Vec<Fact> {
    let mut facts = Vec::new();
    let mut tell = |icon, label: &String, value| {
        facts.push(Fact {
            icon,
            label: label.clone(),
            value,
            wide: false,
        });
    };
    if let Some(heard) = node.last_heard {
        tell("clock", &labels.heard, ago(now, heard));
    }
    // How it is heard, in one place: its signal when directly, which says it
    // was heard directly too; its hops otherwise. Through others, the SNR is
    // the last relay's, not this node's.
    let direct = node.hops_away == Some(0) && !node.via_mqtt;
    match node.snr.filter(|_| direct) {
        // The SNR alone, which the radio always has, in half a row: the RSSI
        // too would take two lines. Hovering the node tells both.
        Some(snr) => tell("antenna-bars-5", &labels.signal, format!("{snr:.1} dB")),
        None => {
            let hops = if node.via_mqtt {
                Some("MQTT".to_owned())
            } else if direct {
                Some(labels.direct.clone())
            } else {
                node.hops_away.map(|h| h.to_string())
            };
            if let Some(hops) = hops {
                tell("route", &labels.hops, hops);
            }
        }
    }
    if let Some(role) = &node.role {
        tell("tag", &labels.role, model(role));
    }
    if let Some(battery) = power(node.battery_level, node.voltage, &labels.external) {
        let plugged = node.battery_level.is_some_and(|level| level > 100);
        tell(
            if plugged { "plug" } else { "battery-2" },
            &labels.battery,
            battery,
        );
    }
    if let Some(busy) = node.channel_utilization {
        tell("activity", &labels.channel, format!("{busy:.1} %"));
    }
    if let Some(position) = &node.position {
        if let Some(altitude) = position.altitude {
            tell("mountain", &labels.altitude, format!("{altitude} m"));
        }
        // Fixed by hand or from a GPS, when the node says.
        if let Some(source) = position.source {
            let how = match source {
                PositionSource::Manual => &labels.manual,
                PositionSource::Internal => &labels.internal,
                PositionSource::External => &labels.external_gps,
            };
            tell("map-pin", &labels.position, how.clone());
        }
        // Most nodes share where they are rounded: how far off it may be.
        if let Some(metres) = position.radius {
            tell("circle-dashed", &labels.precision, radius(metres));
        }
    }
    // The model's name can be long: it takes a row of its own, under the
    // rest, and not half of one.
    if let Some(hardware) = &node.hw_model {
        facts.push(Fact {
            icon: "cpu",
            label: labels.hardware.clone(),
            value: model(hardware),
            wide: true,
        });
    }
    facts
}

/// Something a node's popup tells, with the icon it goes with: the name of
/// one of the bundled Tabler icons.
#[derive(Debug, Serialize)]
struct Fact {
    icon: &'static str,
    label: String,
    value: String,
    /// Whether it takes the popup's whole width, for a value that may be long.
    wide: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_are_coloured_by_how_far_they_are() {
        assert_eq!(kind(Some(1), 1, Some(0), false), "local");
        assert_eq!(kind(Some(1), 2, Some(0), false), "direct");
        assert_eq!(kind(Some(1), 2, Some(2), false), "near");
        assert_eq!(kind(Some(1), 2, Some(3), false), "mid");
        assert_eq!(kind(Some(1), 2, Some(5), false), "far");
        assert_eq!(kind(Some(1), 2, Some(0), true), "far");
        assert_eq!(kind(None, 2, None, false), "far");
    }

    #[test]
    fn hovering_says_how_a_node_is_heard() {
        let words = HopWords {
            direct: "direct".into(),
            one: "1 hop".into(),
            many: "{n} hops".into(),
        };
        let mut node = Node {
            num: 1,
            id: "!00000001".into(),
            long_name: None,
            short_name: None,
            hw_model: None,
            role: None,
            public_key: None,
            licensed: false,
            unmessagable: false,
            position: None,
            last_heard: None,
            snr: Some(11.5),
            rssi: Some(-30),
            hops_away: Some(0),
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
            channel_utilization: None,
        };
        assert_eq!(heard(&node, &words), "SNR 11.5 dB · RSSI -30 dBm");
        node.hops_away = Some(3);
        assert_eq!(heard(&node, &words), "3 hops");
        node.hops_away = Some(1);
        assert_eq!(heard(&node, &words), "1 hop");
        node.via_mqtt = true;
        assert_eq!(heard(&node, &words), "MQTT");
    }

    #[test]
    fn routers_and_bases_are_marked() {
        assert_eq!(role(Some("ROUTER_LATE")), Some("router"));
        assert_eq!(role(Some("REPEATER")), Some("router"));
        assert_eq!(role(Some("CLIENT_BASE")), Some("base"));
        assert_eq!(role(Some("CLIENT_MUTE")), None);
        assert_eq!(role(None), None);
    }

    #[test]
    fn a_route_is_drawn_across_the_nodes_it_cannot_place() {
        let hop = |node: Option<u32>, snr: Option<f32>| Hop { node, snr };
        let hops = [
            hop(Some(1), None),
            hop(Some(2), Some(8.0)),
            hop(Some(3), Some(1.0)),
            hop(None, None),
            hop(Some(5), Some(-9.0)),
        ];
        // Every node is somewhere but the third.
        let at = |node: u32| (node != 3).then(|| (f64::from(node), 0.0));
        let drawn = segments(&hops, &at);
        let told: Vec<_> = drawn
            .iter()
            .map(|s| (s.from.0, s.to.0, s.quality, s.gap))
            .collect();
        assert_eq!(
            told,
            [(1.0, 2.0, "good", false), (2.0, 5.0, "unknown", true)]
        );
    }

    #[test]
    fn a_popup_tells_what_is_known() {
        let node = Node {
            num: 1,
            id: "!00000001".into(),
            long_name: Some("Uno".into()),
            short_name: None,
            hw_model: Some("HELTEC_V3".into()),
            role: Some("CLIENT_MUTE".into()),
            public_key: None,
            licensed: false,
            unmessagable: false,
            position: None,
            last_heard: Some(940),
            snr: Some(6.5),
            rssi: Some(-90),
            hops_away: Some(2),
            via_mqtt: false,
            favorite: false,
            battery_level: Some(80),
            voltage: None,
            channel_utilization: Some(12.5),
        };
        let labels = FactLabels {
            heard: "heard".into(),
            hops: "hops".into(),
            signal: "signal".into(),
            battery: "battery".into(),
            external: "external".into(),
            direct: "direct".into(),
            altitude: "altitude".into(),
            precision: "precision".into(),
            position: "position".into(),
            manual: "manual".into(),
            internal: "gps".into(),
            external_gps: "external gps".into(),
            role: "role".into(),
            channel: "channel".into(),
            hardware: "hardware".into(),
        };
        let told = |node: &Node| -> Vec<String> {
            facts(node, 1000, &labels)
                .into_iter()
                .map(|fact| {
                    let wide = if fact.wide { " (wide)" } else { "" };
                    format!("{}={}{wide}", fact.label, fact.value)
                })
                .collect()
        };
        // Two hops away: no SNR of its own. What is known, in its order, and
        // only the hardware takes a row whole.
        assert_eq!(
            told(&node),
            [
                "heard=1 min",
                "hops=2",
                "role=Client Mute",
                "battery=80 %",
                "channel=12.5 %",
                "hardware=Heltec V3 (wide)"
            ]
        );

        // Heard directly, its signal says so, in the place of its hops.
        let direct = Node {
            hops_away: Some(0),
            ..node
        };
        assert_eq!(told(&direct)[1], "signal=6.5 dB");
    }
}
