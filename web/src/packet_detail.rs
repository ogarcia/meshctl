// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a packet heard says, drawn in its drawer as the rest of the
//! interface draws it: who a node says it is, a text whole, where it says on
//! a small map, its telemetry as a node's figures.

use crate::app::Live;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::Packet;

/// What a packet says, drawn, for the kinds that can be: nothing for the
/// others. `close` closes its drawer, as going to the map does.
#[component]
pub fn Contents(packet: Packet, #[prop(into)] close: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    let from = packet.from;
    let place = place_of(&packet).map(|at| {
        view! { <PacketPlace at from waypoint=waypoint_of(&packet) close /> }
    });
    let telemetry = packet
        .payload
        .as_ref()
        .filter(|_| packet.port.as_deref() == Some("TELEMETRY_APP"))
        .and_then(|payload| crate::node::telemetry_figures(i18n, payload));
    view! {
        {who_of(&packet).map(|who| view! { <PacketWho who from /> })}
        {message_of(&packet).map(|(sender, text)| view! {
            <p class="packet-message">
                {sender.map(|sender| view! { <strong>{sender}</strong> })}
                {text}
            </p>
        })}
        {place}
        {telemetry}
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
    let live = expect_context::<Live>();
    let locale = live.locale();
    let style = crate::mesh::avatar_style(live, from, &who.name);
    let class = if crate::badge::is_emoji(&who.short) {
        "node-avatar emoji"
    } else {
        "node-avatar"
    };
    let role = match who.role.as_str() {
        "UNKNOWN" => None,
        code => Some(crate::names::role_name(locale, code)),
    };
    let more = [who.id, who.model.as_deref().map(crate::mesh::model), role]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
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
        radius: payload["precision_bits"]
            .as_u64()
            .and_then(|bits| u32::try_from(bits).ok())
            .and_then(radius),
    })
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
    fn texts_say_what_they_say() {
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
