// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a packet heard says, drawn in its drawer as the rest of the
//! interface draws it: who a node says it is, a text whole, where it says on
//! a small map with what more it tells of it, a waypoint, its telemetry as a
//! node's figures, a traceroute's ways, the neighbours a node hears, and
//! whether what it answers got there.

use crate::app::Live;
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::node::Tile;
use leptos::prelude::*;
use leptos_i18n::I18nContext;
use meshctl_api::{Hop, Packet};
use serde_json::Value;

const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/circle-check.svg");
const ALERT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/alert-triangle.svg");

/// A relay a traceroute could not tell, as the firmware marks it.
const UNKNOWN_HOP: u64 = 0xffff_ffff;

/// An SNR a traceroute could not tell, as the firmware marks it.
const UNKNOWN_SNR: i64 = -128;

/// What a packet says, drawn, for the kinds that can be: nothing for the
/// others. `close` closes its drawer, as going to the map does.
#[component]
pub fn Contents(packet: Packet, #[prop(into)] close: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    // Its words follow the language: built where that is tracked, as a
    // component's body is not.
    move || {
        let from = packet.from;
        let port = packet.port.as_deref().unwrap_or_default();
        let payload = packet.payload.as_ref();
        let place = place_of(&packet).map(|at| {
            let icon = payload
                .filter(|_| port == "WAYPOINT_APP")
                .map(waypoint_icon);
            view! { <PacketPlace at from waypoint=waypoint_of(&packet) icon close /> }
        });
        let telemetry = payload
            .filter(|_| port == "TELEMETRY_APP")
            .and_then(|payload| crate::node::telemetry_figures(i18n, payload));
        let position = payload
            .filter(|_| port == "POSITION_APP")
            .and_then(|payload| position_figures(i18n, payload));
        let waypoint = payload
            .filter(|_| port == "WAYPOINT_APP")
            .map(|payload| view! { <PacketWaypoint payload=payload.clone() /> });
        let routing = payload
            .filter(|_| port == "ROUTING_APP")
            .map(|payload| view! { <Delivery payload=payload.clone() /> });
        let trace = trace_ways(&packet).map(|ways| view! { <TraceWays ways /> });
        let neighbours =
            (port == "NEIGHBORINFO_APP").then(|| view! { <Neighbours packet=packet.clone() /> });
        view! {
            {who_of(&packet).map(|who| view! { <PacketWho who from /> })}
            {waypoint}
            {message_of(&packet).map(|(sender, text)| view! {
                <p class="packet-message">
                    {sender.map(|sender| view! { <strong>{sender}</strong> })}
                    {text}
                </p>
            })}
            {routing}
            {place}
            {position}
            {telemetry}
            {trace}
            {neighbours}
        }
    }
}

/// What more a position tells of where its node is: how high, how fast and
/// which way, with how many satellites, how far off it may be, and where it
/// comes from. None when it tells none of it.
fn position_figures(i18n: I18nContext<Locale>, payload: &Value) -> Option<AnyView> {
    let tile = |label: &str, value: String| {
        view! { <Tile label=label.to_owned().into_any() value /> }.into_any()
    };
    let int = |key: &str| payload[key].as_i64();
    let mut tiles: Vec<AnyView> = Vec::new();
    if let Some(altitude) = int("altitude") {
        tiles.push(tile(
            t_string!(i18n, node_altitude),
            format!("{altitude} m"),
        ));
    }
    // In metres a second, as the firmware sends it.
    if let Some(speed) = int("ground_speed") {
        #[allow(clippy::cast_precision_loss)]
        let kmh = speed as f64 * 3.6;
        tiles.push(tile(
            t_string!(i18n, packets_speed),
            format!("{kmh:.0} km/h"),
        ));
    }
    // In 1e-5 degrees.
    if let Some(track) = int("ground_track") {
        #[allow(clippy::cast_precision_loss)]
        let degrees = track as f64 * 1e-5;
        tiles.push(tile(
            t_string!(i18n, packets_heading),
            format!("{degrees:.0}°"),
        ));
    }
    if let Some(sats) = int("sats_in_view") {
        tiles.push(tile(t_string!(i18n, packets_satellites), sats.to_string()));
    }
    if let Some(metres) = place_radius(payload) {
        tiles.push(tile(
            t_string!(i18n, node_precision),
            crate::mesh::radius(metres),
        ));
    }
    let source = match payload["location_source"].as_str() {
        Some("LOC_MANUAL") => Some(t_string!(i18n, position_manual)),
        Some("LOC_INTERNAL") => Some(t_string!(i18n, position_internal)),
        Some("LOC_EXTERNAL") => Some(t_string!(i18n, position_external)),
        _ => None,
    };
    if let Some(source) = source {
        tiles.push(tile(t_string!(i18n, node_source), source.to_owned()));
    }
    (!tiles.is_empty())
        .then(|| view! { <div class="node-figures packet-figures">{tiles}</div> }.into_any())
}

/// A waypoint, as its pin on the map shows it: its emoji, its name and what
/// it says of itself, when it expires and whether it is locked; or that it
/// deletes the one it names, for everyone.
#[component]
fn PacketWaypoint(payload: Value) -> impl IntoView {
    let i18n = use_i18n();
    let icon = waypoint_icon(&payload);
    let name = payload["name"].as_str().unwrap_or_default().to_owned();
    let description = payload["description"]
        .as_str()
        .filter(|d| !d.is_empty())
        .map(str::to_owned);
    let expire = payload["expire"].as_i64().unwrap_or(0);
    // Deleted, it is sent again expired, and with nowhere.
    let deleted = expire == 1 || payload.get("latitude_i").is_none();
    let locked = payload["locked_to"].as_u64().is_some_and(|n| n != 0);
    // Its words follow the language.
    let about = move || {
        let mut about = vec![if deleted {
            t_string!(i18n, packets_waypoint_deleted).to_owned()
        } else if expire == 0 {
            format!(
                "{}: {}",
                t_string!(i18n, waypoint_expires),
                t_string!(i18n, waypoint_never)
            )
        } else {
            format!(
                "{}: {}",
                t_string!(i18n, waypoint_expires),
                crate::packets::when(expire)
            )
        }];
        if locked {
            about.push(t_string!(i18n, waypoint_locked).to_owned());
        }
        about.join(" · ")
    };
    view! {
        <div class="packet-who packet-waypoint">
            <span class="waypoint-icon">{icon}</span>
            <div class="names">
                <span class="name">{name}</span>
                {description.map(|d| view! { <span class="about">{d}</span> })}
                <span class="about dim">{about}</span>
            </div>
        </div>
    }
}

/// A waypoint's emoji, as its code point says; a pin when it has none.
fn waypoint_icon(payload: &Value) -> String {
    payload["icon"]
        .as_u64()
        .and_then(|code| char::from_u32(u32::try_from(code).ok()?))
        .map_or_else(|| "📍".to_owned(), String::from)
}

/// Whether what a routing packet answers got there: confirmed, or why not,
/// in words.
#[component]
fn Delivery(payload: Value) -> impl IntoView {
    let i18n = use_i18n();
    // Its reason left out is the first, none.
    let code = payload["error_reason"]
        .as_str()
        .unwrap_or("NONE")
        .to_owned();
    let ok = code == "NONE";
    // Its words follow the language.
    let words = move || {
        if ok {
            t_string!(i18n, packets_ack).to_owned()
        } else {
            crate::names::routing_error(i18n.get_locale(), &code)
        }
    };
    view! {
        <p class="packet-status" class:ok=ok class:bad=!ok>
            <span class="icon" inner_html=if ok { CHECK } else { ALERT }></span>
            <span>{words}</span>
        </p>
    }
}

/// A traceroute's ways, as the packet heard tells them: there, and back once
/// it is answered; or, on its way there, as far as it went.
#[derive(Clone, Debug, PartialEq)]
struct Ways {
    towards: Vec<Hop>,
    back: Option<Vec<Hop>>,
}

/// The ways a traceroute heard tells: its answer, from whoever asked to the
/// node traced and back; its request, from whoever asked as far as it went.
fn trace_ways(packet: &Packet) -> Option<Ways> {
    if packet.port.as_deref() != Some("TRACEROUTE_APP") {
        return None;
    }
    let payload = packet.payload.as_ref()?;
    let list = |key: &str| -> Vec<i64> {
        payload[key]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    let hops = |first: u32, relays: &[i64], last: Option<u32>, snrs: &[i64]| -> Vec<Hop> {
        let rest = relays
            .iter()
            .map(|&n| {
                u32::try_from(n)
                    .ok()
                    .filter(|&n| u64::from(n) != UNKNOWN_HOP)
            })
            .chain(last.map(Some))
            .enumerate()
            .map(|(i, node)| {
                // Scaled by four.
                #[allow(clippy::cast_precision_loss)]
                let snr = snrs
                    .get(i)
                    .filter(|&&s| s != UNKNOWN_SNR)
                    .map(|&s| s as f32 / 4.0);
                Hop::known(node, snr)
            });
        std::iter::once(Hop::known(Some(first), None))
            .chain(rest)
            .collect()
    };
    let (route, snr_towards) = (list("route"), list("snr_towards"));
    Some(if packet.request_id.is_some() {
        // The answer, from the node traced to whoever asked.
        let (origin, target) = (packet.to, packet.from);
        Ways {
            towards: hops(origin, &route, Some(target), &snr_towards),
            back: Some(hops(
                target,
                &list("route_back"),
                Some(origin),
                &list("snr_back"),
            )),
        }
    } else {
        Ways {
            towards: hops(packet.from, &route, None, &snr_towards),
            back: None,
        }
    })
}

/// A traceroute's ways, node by node with how well each hop was heard, as a
/// trace on a node's page shows them; and on a small map, where its nodes
/// can be placed.
#[component]
fn TraceWays(ways: Ways) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let local = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    let all: Vec<Hop> = ways
        .towards
        .iter()
        .chain(ways.back.iter().flatten().skip(1))
        .cloned()
        .collect();
    let points = Signal::derive(move || {
        live.nodes.with(|nodes| {
            let mut points = Vec::new();
            let mut gap = false;
            for hop in &all {
                let at = hop.node.and_then(|num| {
                    let node = nodes.get(&num)?;
                    let at = node.position?;
                    Some(crate::map::RoutePoint {
                        lat: at.latitude,
                        lon: at.longitude,
                        pin: crate::mesh::pin(node, Some(num) == local),
                        gap: gap && !points.is_empty(),
                        guess: false,
                    })
                });
                match at {
                    Some(point) => {
                        points.push(point);
                        gap = false;
                    }
                    None => gap = true,
                }
            }
            points
        })
    });
    let answered = ways.back.is_some();
    let there = ways
        .towards
        .len()
        .saturating_sub(if answered { 2 } else { 1 });
    let towards = ways.towards;
    let back = ways.back.map(|back| {
        let count = back.len().saturating_sub(2);
        view! {
            <h3>{t!(i18n, trace_back)} " (" {count} ")"</h3>
            <crate::traces::Route hops=back />
        }
    });
    view! {
        <section class="routes packet-trace">
            <h3>
                {if answered { t!(i18n, trace_towards).into_any() } else { t!(i18n, packets_trace_so_far).into_any() }}
                " (" {there} ")"
            </h3>
            <crate::traces::Route hops=towards />
            {back}
            {move || {
                (points.with(Vec::len) >= 2).then(|| view! { <crate::map::RouteMap points /> })
            }}
        </section>
    }
}

/// The nodes a node says it hears directly, as its page lists them.
#[component]
fn Neighbours(packet: Packet) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let all = RwSignal::new(false);
    let count = packet
        .payload
        .as_ref()
        .and_then(|p| p["neighbors"].as_array().map(Vec::len))
        .unwrap_or(0);
    let list = crate::node::neighbour_list(i18n, live, Some(&packet), all);
    view! {
        <section class="packet-neighbours">
            <h3>{t!(i18n, node_neighbors)} " (" {count} ")"</h3>
            {list}
        </section>
    }
}

/// Where a position or a waypoint heard says, on a small map, its pin in the
/// colour of its sender as the map draws it. A click shows it on the big
/// map: the waypoint itself if it is still there, or the place.
#[component]
fn PacketPlace(
    at: Place,
    from: u32,
    waypoint: Option<u32>,
    /// A waypoint's emoji, for its pin.
    icon: Option<String>,
    #[prop(into)] close: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let local = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    let pin = live.nodes.with_untracked(|nodes| {
        nodes.get(&from).map_or("--mc-pin-far", |node| {
            crate::mesh::pin(node, local == Some(from))
        })
    });
    let label = Signal::derive(move || t_string!(i18n, node_on_map).to_owned());
    view! {
        <crate::map::PlaceMap
            lat=at.lat
            lon=at.lon
            radius=at.radius
            pin
            icon
            label
            open=move |()| {
                close.run(());
                let kept = waypoint.filter(|id| {
                    live.waypoints.with_untracked(|w| w.iter().any(|w| w.id == *id))
                });
                match kept {
                    Some(id) => live.show_waypoint(id),
                    None => live.show_place(at.lat, at.lon),
                }
            }
        />
    }
}

/// Who a node says it is, as its node info or its advert tells: its names,
/// and what more it says of itself.
#[derive(Clone, Debug, PartialEq)]
struct Who {
    name: String,
    /// Its avatar's letters, its name's first ones when it has none.
    short: String,
    /// Its id, Meshtastic's.
    id: Option<String>,
    /// Its hardware's model, as the protobufs name it.
    model: Option<String>,
    /// Its role, as the protobufs name it, MeshCore's kinds among them.
    role: String,
}

/// Who a node info or a MeshCore advert says its sender is.
fn who_of(packet: &Packet) -> Option<Who> {
    let payload = packet.payload.as_ref()?;
    let text = |key: &str| {
        payload[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match packet.port.as_deref()? {
        "NODEINFO_APP" => {
            let name = text("long_name")?;
            Some(Who {
                short: text("short_name").unwrap_or_else(|| crate::badge::initials(&name)),
                name,
                id: text("id"),
                model: text("hw_model"),
                // What a protobuf leaves at its first value its JSON leaves
                // out.
                role: text("role").unwrap_or_else(|| "CLIENT".to_owned()),
            })
        }
        "ADVERT" => {
            let name = text("name")?;
            let role = match payload["kind"].as_str() {
                Some("companion") => "CHAT",
                Some("repeater") => "REPEATER",
                Some("room") => "ROOM",
                Some("sensor") => "SENSOR",
                _ => "UNKNOWN",
            };
            Some(Who {
                short: crate::badge::initials(&name),
                name,
                id: None,
                model: None,
                role: role.to_owned(),
            })
        }
        _ => None,
    }
}

/// What a text says, whole, and who wrote it when it says so apart: a
/// MeshCore channel's.
fn message_of(packet: &Packet) -> Option<(Option<String>, String)> {
    let payload = packet.payload.as_ref()?;
    if let Some(text) = payload.as_str() {
        return (!text.is_empty()).then(|| (None, text.to_owned()));
    }
    if packet.port.as_deref() == Some("NODE_STATUS_APP") {
        let status = payload["status"].as_str().filter(|s| !s.is_empty())?;
        return Some((None, status.to_owned()));
    }
    if packet.port.as_deref() != Some("GRP_TXT") {
        return None;
    }
    let text = payload["text"].as_str().filter(|t| !t.is_empty())?;
    Some((
        payload["sender"].as_str().map(str::to_owned),
        text.to_owned(),
    ))
}

/// Who a node says it is, with its avatar coloured as everywhere: its name,
/// and under it its id, its model and its role.
#[component]
fn PacketWho(who: Who, from: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let style = crate::mesh::avatar_style(live, from, &who.name);
    let class = if crate::badge::is_emoji(&who.short) {
        "node-avatar emoji"
    } else {
        "node-avatar"
    };
    // Its role's words follow the language.
    let (id, model, role) = (
        who.id,
        who.model.as_deref().map(crate::mesh::model),
        who.role,
    );
    let more = move || {
        let role = (role != "UNKNOWN").then(|| crate::names::role_name(i18n.get_locale(), &role));
        [id.clone(), model.clone(), role]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    view! {
        <div class="packet-who">
            <span class=class style=style>{crate::badge::fit(&who.short)}</span>
            <div class="names">
                <span class="name">{who.name}</span>
                <span class="about dim">{more}</span>
            </div>
        </div>
    }
}

/// Where a packet says, in degrees, and how far off it may be.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    lat: f64,
    lon: f64,
    radius: Option<u32>,
}

/// Where a position, a waypoint or a MeshCore advert says: none for the
/// others, or for one that says nowhere, zero for both as an unset one
/// reads.
fn place_of(packet: &Packet) -> Option<Place> {
    let port = packet.port.as_deref()?;
    let payload = packet.payload.as_ref()?;
    if port == "ADVERT" {
        let (lat, lon) = (
            payload["latitude"].as_f64()?,
            payload["longitude"].as_f64()?,
        );
        return (lat != 0.0 || lon != 0.0).then_some(Place {
            lat,
            lon,
            radius: None,
        });
    }
    if port != "POSITION_APP" && port != "WAYPOINT_APP" {
        return None;
    }
    let (lat, lon) = (
        payload["latitude_i"].as_i64()?,
        payload["longitude_i"].as_i64()?,
    );
    if lat == 0 && lon == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(Place {
        lat: lat as f64 * 1e-7,
        lon: lon as f64 * 1e-7,
        radius: place_radius(payload),
    })
}

/// How far off a position may be, in metres, as its precision says.
fn place_radius(payload: &Value) -> Option<u32> {
    payload["precision_bits"]
        .as_u64()
        .and_then(|bits| u32::try_from(bits).ok())
        .and_then(radius)
}

/// A waypoint's id, for one heard.
fn waypoint_of(packet: &Packet) -> Option<u32> {
    (packet.port.as_deref() == Some("WAYPOINT_APP"))
        .then(|| packet.payload.as_ref()?["id"].as_u64())
        .flatten()
        .and_then(|id| u32::try_from(id).ok())
}

/// How far, in metres, a position shared rounded may be from where the node
/// is, as the daemon tells it for a node's: half the side of the cell its
/// precision rounds to. None when it is exact (32 bits) or does not say (0).
fn radius(precision_bits: u32) -> Option<u32> {
    if !(1..32).contains(&precision_bits) {
        return None;
    }
    // A degree of latitude is about 111 320 m; coordinates go by 1e-7 degrees.
    let side = f64::from(1_u32 << (32 - precision_bits)) * 1e-7 * 111_320.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((side / 2.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::BROADCAST;
    use serde_json::Value;

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
    fn positions_and_waypoints_have_a_place() {
        let position = serde_json::json!({
            "latitude_i": 433_000_000, "longitude_i": -84_000_000, "precision_bits": 13
        });
        let place = place_of(&packet("POSITION_APP", position)).unwrap();
        assert!((place.lat - 43.3).abs() < 1e-9 && (place.lon + 8.4).abs() < 1e-9);
        assert_eq!(place.radius, Some(2918));
        let waypoint = serde_json::json!({
            "id": 77, "latitude_i": 433_000_000, "longitude_i": -84_000_000, "name": "Faro"
        });
        let heard = packet("WAYPOINT_APP", waypoint);
        assert_eq!(place_of(&heard).unwrap().radius, None);
        assert_eq!(waypoint_of(&heard), Some(77));
        // An unset one, and those of other kinds, say nowhere.
        let unset = serde_json::json!({"latitude_i": 0, "longitude_i": 0});
        assert_eq!(place_of(&packet("POSITION_APP", unset)), None);
        let text = packet("TEXT_MESSAGE_APP", "43.3, -8.4".into());
        assert_eq!(place_of(&text), None);
        assert_eq!(waypoint_of(&text), None);
        // A MeshCore advert, in degrees, when it says.
        let advert = serde_json::json!({"name": "Torre", "kind": "repeater", "latitude": 43.3, "longitude": -8.4});
        assert_eq!(place_of(&packet("ADVERT", advert)).unwrap().lat, 43.3);
        let nowhere = serde_json::json!({"name": "Torre", "latitude": null, "longitude": null});
        assert_eq!(place_of(&packet("ADVERT", nowhere)), None);
    }

    #[test]
    fn node_infos_and_adverts_say_who() {
        let info = serde_json::json!({
            "id": "!0badcafe", "long_name": "Fake neighbour", "short_name": "NBR",
            "hw_model": "HELTEC_V3", "role": "ROUTER"
        });
        let who = who_of(&packet("NODEINFO_APP", info)).unwrap();
        assert_eq!((who.short.as_str(), who.role.as_str()), ("NBR", "ROUTER"));
        // A client's role is its protobuf's first, left out.
        let client = serde_json::json!({"long_name": "Pepe"});
        assert_eq!(
            who_of(&packet("NODEINFO_APP", client)).unwrap().role,
            "CLIENT"
        );
        let advert = serde_json::json!({"name": "Torre Hércules", "kind": "room"});
        let who = who_of(&packet("ADVERT", advert)).unwrap();
        assert_eq!((who.short.as_str(), who.role.as_str()), ("TH", "ROOM"));
    }

    #[test]
    fn traceroutes_say_their_ways() {
        // Asked by 1 of 9, through 5 and an unknown relay there, and 7 back.
        let answer = serde_json::json!({
            "route": [5, 0xffff_ffff_u32], "snr_towards": [24, -128, 10],
            "route_back": [7], "snr_back": [-8]
        });
        let mut heard = packet("TRACEROUTE_APP", answer);
        heard.from = 9;
        heard.to = 1;
        heard.request_id = Some(42);
        let ways = trace_ways(&heard).unwrap();
        let nodes = |hops: &[Hop]| hops.iter().map(|h| h.node).collect::<Vec<_>>();
        let snrs = |hops: &[Hop]| hops.iter().map(|h| h.snr).collect::<Vec<_>>();
        assert_eq!(nodes(&ways.towards), [Some(1), Some(5), None, Some(9)]);
        assert_eq!(snrs(&ways.towards), [None, Some(6.0), None, Some(2.5)]);
        let back = ways.back.unwrap();
        assert_eq!(nodes(&back), [Some(9), Some(7), Some(1)]);
        assert_eq!(snrs(&back), [None, Some(-2.0), None]);
        // On its way there, as far as it went.
        let asked = serde_json::json!({"route": [5], "snr_towards": [24]});
        let ways = trace_ways(&packet("TRACEROUTE_APP", asked)).unwrap();
        assert_eq!(nodes(&ways.towards), [Some(1), Some(5)]);
        assert_eq!(ways.back, None);
    }

    #[test]
    fn texts_say_what_they_say() {
        let status = serde_json::json!({"status": "De ruta"});
        assert_eq!(
            message_of(&packet("NODE_STATUS_APP", status)),
            Some((None, "De ruta".to_owned()))
        );
        let text = packet("TEXT_MESSAGE_APP", "hola".into());
        assert_eq!(message_of(&text), Some((None, "hola".to_owned())));
        let channel = serde_json::json!({"sender": "Pepe", "text": "Bos días"});
        assert_eq!(
            message_of(&packet("GRP_TXT", channel)),
            Some((Some("Pepe".to_owned()), "Bos días".to_owned()))
        );
        let position = serde_json::json!({"latitude_i": 1});
        assert_eq!(message_of(&packet("POSITION_APP", position)), None);
    }
}
