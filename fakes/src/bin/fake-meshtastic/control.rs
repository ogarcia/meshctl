// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The control port: lines that play what another app or the mesh would
//! do to the radio.

use crate::neighbour::{Answers, history};
use crate::radio::{BROADCAST, Event, ME, NEIGHBOUR, REPEATER, Radio, fresh_id};
use meshctl_fakes::{now, random};
use meshctl_proto::meshtastic::{self as pb, MeshPacket, PortNum, mesh_packet};
use prost::Message;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Does what a line of the control port says; what to answer it.
pub fn control(line: &str, radio: &Mutex<Radio>, events: &broadcast::Sender<Event>) -> String {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0x5000_0000);
    let mut words = line.trim().splitn(3, ' ');
    let slot = |word: Option<&str>| {
        word.and_then(|w| w.parse::<i32>().ok())
            .filter(|s| (0..8).contains(s))
    };
    match words.next() {
        Some("hear") => {
            let (Some(channel), Some(text)) = (slot(words.next()), words.next()) else {
                return "usage: hear <slot> <text>".into();
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as u32);
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                channel: channel as u32,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now),
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::TextMessageApp as i32,
                    payload: text.as_bytes().to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("heard on slot {channel}")
        }
        // Waypoints shared by the neighbour and the repeater, as the apps
        // share them: one open to all, fenced by a circle; one locked to the
        // repeater, fenced by a box; one that expires in an hour.
        Some("waypoints") => {
            let Some(channel) = slot(words.next()) else {
                return "usage: waypoints <slot>".into();
            };
            let shared = [
                (
                    NEIGHBOUR,
                    pb::Waypoint {
                        id: 0x0000_cafe,
                        latitude_i: Some(404_200_000),
                        longitude_i: Some(-37_080_000),
                        name: "Punto de encuentro".into(),
                        description: "Junto a la fuente, a las diez".into(),
                        icon: 0x1F91D,
                        geofence_radius: 250,
                        notify_on_enter: true,
                        ..Default::default()
                    },
                ),
                (
                    REPEATER,
                    pb::Waypoint {
                        id: 0x0000_0de5,
                        latitude_i: Some(404_330_000),
                        longitude_i: Some(-37_120_000),
                        name: "Caseta del repetidor".into(),
                        description: "Llave en el bar de abajo".into(),
                        icon: 0x1F4E1,
                        locked_to: REPEATER,
                        bounding_box: Some(pb::BoundingBox {
                            longitude_west_i: -37_160_000,
                            latitude_south_i: 404_300_000,
                            longitude_east_i: -37_080_000,
                            latitude_north_i: 404_360_000,
                        }),
                        notify_on_exit: true,
                        ..Default::default()
                    },
                ),
                (
                    NEIGHBOUR,
                    pb::Waypoint {
                        id: 0x0000_beef,
                        latitude_i: Some(404_150_000),
                        longitude_i: Some(-37_000_000),
                        expire: now() + 3_600,
                        name: "Mercadillo".into(),
                        description: "Solo esta mañana".into(),
                        icon: 0x1F6D2,
                        ..Default::default()
                    },
                ),
            ];
            for (from, waypoint) in &shared {
                let packet = MeshPacket {
                    from: *from,
                    to: BROADCAST,
                    channel: channel as u32,
                    id: fresh_id(&NEXT_ID),
                    rx_time: Some(now()),
                    rx_snr: 6.0,
                    hop_limit: 3,
                    hop_start: 3,
                    payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                        portnum: PortNum::WaypointApp as i32,
                        payload: waypoint.encode_to_vec(),
                        ..Default::default()
                    })),
                    ..Default::default()
                };
                let _ = events.send(Event::Heard(packet));
            }
            format!("{} waypoints on slot {channel}", shared.len())
        }
        // The neighbour somewhere else, as its position says: to see what
        // follows it, as fences that watch it.
        Some("move") => {
            let (Some(lat), Some(lon)) = (
                words.next().and_then(|w| w.parse::<f64>().ok()),
                words.next().and_then(|w| w.parse::<f64>().ok()),
            ) else {
                return "usage: move <lat> <lon>".into();
            };
            #[allow(clippy::cast_possible_truncation)]
            let scaled = |degrees: f64| (degrees * 1e7).round() as i32;
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now()),
                rx_snr: 6.0,
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::PositionApp as i32,
                    payload: pb::Position {
                        latitude_i: Some(scaled(lat)),
                        longitude_i: Some(scaled(lon)),
                        time: now(),
                        precision_bits: 32,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("the neighbour is at {lat}, {lon}")
        }
        // A waypoint deleted for everyone by whoever shared it, by its id in
        // hex: sent again, expired.
        Some("unwaypoint") => {
            let Some(id) = words.next().and_then(|w| u32::from_str_radix(w, 16).ok()) else {
                return "usage: unwaypoint <id in hex>".into();
            };
            let from = if id == 0x0de5 { REPEATER } else { NEIGHBOUR };
            let packet = MeshPacket {
                from,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now()),
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::WaypointApp as i32,
                    payload: pb::Waypoint {
                        id,
                        expire: 1,
                        locked_to: if from == REPEATER { REPEATER } else { 0 },
                        ..Default::default()
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("waypoint {id:08x} deleted by !{from:08x}")
        }
        Some("past") => {
            let Some(channel) = slot(words.next()) else {
                return "usage: past <slot>".into();
            };
            // Days ago, and the hour of that day, as it was heard then.
            let said = [
                (400, 9, "Hace más de un año"),
                (400, 18, "Aquel mismo día, por la tarde"),
                (20, 11, "Hace unas semanas"),
                (3, 8, "Hace unos días, por la mañana"),
                (3, 22, "Ese día, de noche"),
                (1, 13, "Ayer a mediodía"),
                (0, 0, "Hoy mismo"),
            ];
            let today = now() - now() % 86_400;
            for (days, hour, text) in said {
                let at = if days == 0 {
                    now()
                } else {
                    today - days * 86_400 + hour * 3_600 + random(1_800) as u32
                };
                let packet = MeshPacket {
                    from: NEIGHBOUR,
                    to: BROADCAST,
                    channel: channel as u32,
                    id: fresh_id(&NEXT_ID),
                    rx_time: Some(at),
                    rx_snr: 6.0,
                    hop_limit: 3,
                    hop_start: 3,
                    payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                        portnum: PortNum::TextMessageApp as i32,
                        payload: text.as_bytes().to_vec(),
                        ..Default::default()
                    })),
                    ..Default::default()
                };
                let _ = events.send(Event::Heard(packet));
            }
            format!("{} messages from the past on slot {channel}", said.len())
        }
        Some("chat") => {
            let Some(channel) = slot(words.next()) else {
                return "usage: chat <slot>".into();
            };
            // From whom, how far (hops left of those it started with, the
            // last relay), and what: as many ways as a message comes.
            struct Example {
                from: u32,
                hops: (u32, u32),
                relay: u32,
                mqtt: bool,
                text: &'static str,
                reply: bool,
                emoji: bool,
            }
            let example = |from, hops, relay, mqtt, text| Example {
                from,
                hops,
                relay,
                mqtt,
                text,
                reply: false,
                emoji: false,
            };
            let mut examples = vec![
                example(NEIGHBOUR, (3, 3), 0, false, "Hola, te oigo directo"),
                example(0x0dec_0de6, (2, 3), 0xfe, false, "Por aquí, a un salto"),
                example(
                    0x0dec_0de7,
                    (0, 3),
                    0xca,
                    false,
                    "Llego justo: tres saltos de tres",
                ),
                example(0x0dec_0de8, (5, 7), 0xfe, false, "Dos saltos de siete"),
                example(0x0dec_0de9, (3, 3), 0, true, "Desde MQTT"),
            ];
            let last = radio.lock().unwrap().last_text;
            if last.is_some() {
                examples.push(Example {
                    reply: true,
                    ..example(
                        NEIGHBOUR,
                        (3, 3),
                        0,
                        false,
                        "Respondo a lo último que escribiste",
                    )
                });
                for (from, emoji) in [(NEIGHBOUR, "👍"), (0x0dec_0de6, "❤️")] {
                    examples.push(Example {
                        reply: true,
                        emoji: true,
                        ..example(from, (2, 3), 0xfe, false, emoji)
                    });
                }
            }
            let count = examples.len();
            let events = events.clone();
            tokio::spawn(async move {
                for example in examples {
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                    let packet = MeshPacket {
                        from: example.from,
                        to: BROADCAST,
                        channel: channel as u32,
                        id: fresh_id(&NEXT_ID),
                        rx_time: Some(now()),
                        rx_snr: 2.0 + random(80) as f32 / 10.0,
                        rx_rssi: Some(-60 - random(40) as i32),
                        hop_limit: example.hops.0,
                        hop_start: example.hops.1,
                        relay_node: example.relay,
                        via_mqtt: example.mqtt,
                        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                            portnum: PortNum::TextMessageApp as i32,
                            payload: example.text.as_bytes().to_vec(),
                            reply_id: if example.reply {
                                last.unwrap_or_default()
                            } else {
                                0
                            },
                            emoji: u32::from(example.emoji),
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    let _ = events.send(Event::Heard(packet));
                }
            });
            format!("{count} messages on slot {channel}, one every 1.5 s")
        }
        // Many nodes heard, about Madrid, at every distance, of every role and
        // heard at every age; the link dropped to have them read. 0, none.
        Some("crowd") => {
            let Some(count) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return format!("usage: crowd <nodes, up to {}>", crate::crowd::MOST);
            };
            let mut radio = radio.lock().unwrap();
            radio.crowd = crate::crowd::crowd(count);
            let told = radio.crowd.len();
            drop(radio);
            let _ = events.send(Event::Drop);
            format!("a crowd of {told}; dropped to have them read")
        }
        Some("fill") => {
            // Every slot a channel, as a radio that has them all; one with
            // a long name, to be cut.
            let names = [
                "Galicia",
                "Emergencias",
                "Montaña",
                "Vecinos",
                "Radioaficionados",
                "Pruebas con un nombre larguísimo",
            ];
            let mut radio = radio.lock().unwrap();
            for (index, name) in (2..8).zip(names) {
                radio.channels.insert(
                    index,
                    pb::Channel {
                        index,
                        role: pb::channel::Role::Secondary as i32,
                        settings: Some(pb::ChannelSettings {
                            name: name.into(),
                            psk: vec![index as u8; 32],
                            ..Default::default()
                        }),
                    },
                );
            }
            drop(radio);
            let _ = events.send(Event::Drop);
            "slots 2 to 7 are channels now; dropped to have them read".into()
        }
        Some("dms") => {
            // Who they are, and what each writes to the radio.
            let nodes: [(u32, &str, &str, &[&str]); 9] = [
                (
                    0x0dec_1001,
                    "Ana Coruña",
                    "ANA",
                    &["Hola, ¿me lees?", "Te oigo bien desde casa"],
                ),
                (
                    0x0dec_1002,
                    "Bici de Bea",
                    "BEA",
                    &["Saliendo hacia el monte"],
                ),
                (
                    0x0dec_1003,
                    "Repetidor Monte Xalo",
                    "XALO",
                    &["Batería al 80 %", "Todo en orden"],
                ),
                (
                    0x0dec_1004,
                    "Coche de Luis",
                    "LUIS",
                    &["En la autopista, llego en media hora"],
                ),
                (
                    0x0dec_1005,
                    "Base Ferrol",
                    "FERR",
                    &["¿Alguien en la frecuencia?"],
                ),
                (
                    0x0dec_1006,
                    "María",
                    "MAR",
                    &["Gracias por el aviso", "Nos vemos mañana", "👍"],
                ),
                (
                    0x0dec_1007,
                    "Estación meteorológica",
                    "METE",
                    &["Mañana lluvia"],
                ),
                (
                    0x0dec_1008,
                    "Un nodo con un nombre larguísimo para ver cómo se corta",
                    "LARG",
                    &["Probando nombres largos"],
                ),
                (
                    0x0dec_1009,
                    "Pedro",
                    "PED",
                    &["¿Quedamos para probar antenas?"],
                ),
            ];
            let count = nodes
                .iter()
                .map(|(_, _, _, said)| said.len())
                .sum::<usize>();
            let events = events.clone();
            tokio::spawn(async move {
                for (num, long, short, said) in nodes {
                    let user = pb::User {
                        id: format!("!{num:08x}"),
                        long_name: long.into(),
                        short_name: short.into(),
                        hw_model: pb::HardwareModel::HeltecV3 as i32,
                        public_key: num.to_be_bytes().repeat(8),
                        ..Default::default()
                    };
                    let heard = |to, port: PortNum, payload: Vec<u8>| MeshPacket {
                        from: num,
                        to,
                        channel: 0,
                        id: fresh_id(&NEXT_ID),
                        rx_time: Some(now()),
                        rx_snr: 2.0 + random(80) as f32 / 10.0,
                        rx_rssi: Some(-60 - random(40) as i32),
                        hop_limit: 3 - (num % 3),
                        hop_start: 3,
                        payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                            portnum: port as i32,
                            payload,
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    let _ = events.send(Event::Heard(heard(
                        BROADCAST,
                        PortNum::NodeinfoApp,
                        user.encode_to_vec(),
                    )));
                    for text in said {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let _ = events.send(Event::Heard(heard(
                            ME,
                            PortNum::TextMessageApp,
                            text.as_bytes().to_vec(),
                        )));
                    }
                }
            });
            format!("{count} direct messages from {} nodes", nodes.len())
        }
        Some("twin") => {
            let (Some(from), Some(to)) = (slot(words.next()), slot(words.next())) else {
                return "usage: twin <from> <to>".into();
            };
            let mut radio = radio.lock().unwrap();
            let Some(mut channel) = radio.channels.get(&from).cloned() else {
                return "no such channel".into();
            };
            channel.index = to;
            channel.role = if to == 0 {
                pb::channel::Role::Primary
            } else {
                pb::channel::Role::Secondary
            } as i32;
            radio.channels.insert(to, channel);
            format!("slot {to} is slot {from} again; drop to have it read")
        }
        Some("off") => {
            let Some(index) = slot(words.next()).filter(|s| *s != 0) else {
                return "usage: off <slot>, not 0".into();
            };
            radio.lock().unwrap().channels.insert(
                index,
                pb::Channel {
                    index,
                    role: pb::channel::Role::Disabled as i32,
                    settings: Some(pb::ChannelSettings::default()),
                },
            );
            format!("slot {index} off; drop to have it read")
        }
        Some("drop") => {
            let _ = events.send(Event::Drop);
            "dropped".into()
        }
        Some("place") => {
            // Where it answers it is when asked, of its own accord.
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now()),
                rx_snr: 7.5,
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::PositionApp as i32,
                    payload: pb::Position {
                        latitude_i: Some(404_168_000 + random(2_000) as i32),
                        longitude_i: Some(-37_038_000 + random(2_000) as i32),
                        altitude: Some(650 + random(20) as i32),
                        time: now(),
                        location_source: pb::position::LocSource::LocInternal as i32,
                        precision_bits: 32,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            "the neighbour says where it is".into()
        }
        Some("status") => {
            let text = words.collect::<Vec<_>>().join(" ");
            let packet = MeshPacket {
                from: NEIGHBOUR,
                to: BROADCAST,
                id: fresh_id(&NEXT_ID),
                rx_time: Some(now()),
                rx_snr: 7.5,
                hop_limit: 3,
                hop_start: 3,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::NodeStatusApp as i32,
                    payload: pb::StatusMessage {
                        status: text.clone(),
                    }
                    .encode_to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let _ = events.send(Event::Heard(packet));
            format!("the neighbour says «{text}»")
        }
        Some("own") => {
            // As the firmware tells its client now and then: its device
            // telemetry, sent to the mesh too, and how it hears it.
            let now = now();
            let variants = [
                pb::telemetry::Variant::DeviceMetrics(pb::DeviceMetrics {
                    battery_level: Some(20 + random(80) as u32),
                    voltage: Some(3.5 + random(70) as f32 / 100.0),
                    channel_utilization: Some(random(200) as f32 / 10.0),
                    air_util_tx: Some(random(50) as f32 / 10.0),
                    uptime_seconds: Some(3_600 + random(86_400) as u32),
                }),
                pb::telemetry::Variant::LocalStats(pb::LocalStats {
                    uptime_seconds: 3_600 + random(86_400) as u32,
                    channel_utilization: random(200) as f32 / 10.0,
                    air_util_tx: random(50) as f32 / 10.0,
                    num_packets_tx: 100 + random(900) as u32,
                    num_packets_rx: 1_000 + random(9_000) as u32,
                    num_packets_rx_bad: random(50) as u32,
                    num_online_nodes: 5 + random(30) as u32,
                    num_total_nodes: 40 + random(60) as u32,
                    noise_floor: -110 + random(15) as i32,
                    ..Default::default()
                }),
            ];
            for variant in variants {
                let to = if matches!(variant, pb::telemetry::Variant::LocalStats(_)) {
                    ME
                } else {
                    BROADCAST
                };
                let packet = MeshPacket {
                    from: ME,
                    to,
                    id: fresh_id(&NEXT_ID),
                    rx_time: Some(now),
                    payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                        portnum: PortNum::TelemetryApp as i32,
                        payload: pb::Telemetry {
                            time: now,
                            variant: Some(variant),
                        }
                        .encode_to_vec(),
                        ..Default::default()
                    })),
                    ..Default::default()
                };
                let _ = events.send(Event::Heard(packet));
            }
            "its own telemetry and stats, told".into()
        }
        Some("history") => {
            let Some(hours) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return "usage: history <hours>".into();
            };
            let packets = history(hours);
            let count = packets.len();
            // A few at a time, as clients keep only so many events behind.
            let events = events.clone();
            tokio::spawn(async move {
                for packet in packets {
                    let _ = events.send(Event::Heard(packet));
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            format!("{count} packets of the neighbour's last {hours} hours, on their way")
        }
        Some("answers") => {
            let answers = match words.next() {
                Some("always") => Answers::Always,
                Some("sometimes") => Answers::Sometimes,
                Some("never") => Answers::Never,
                _ => return "usage: answers always|sometimes|never".into(),
            };
            radio.lock().unwrap().answers = answers;
            format!("the neighbour answers: {answers:?}")
        }
        Some("limits") => {
            let limits = match words.next() {
                Some("on") => true,
                Some("off") => false,
                _ => return "usage: limits on|off".into(),
            };
            let mut radio = radio.lock().unwrap();
            radio.limits = limits;
            radio.answered.clear();
            format!("the neighbour keeps from answering too often: {limits}")
        }
        Some("admin") => {
            let allowed = match words.next() {
                Some("allow") => true,
                Some("deny") => false,
                _ => return "usage: admin allow|deny".into(),
            };
            radio.lock().unwrap().admin_key = allowed;
            format!("the neighbour takes the radio's key as an admin key: {allowed}")
        }
        _ => "commands: hear <slot> <text>, past <slot>, chat <slot>, dms, fill, \
             twin <from> <to>, off <slot>, crowd <nodes>, own, place, move <lat> <lon>, status [<text>], \
             waypoints <slot>, unwaypoint <id>, history <hours>, \
             answers always|sometimes|never, limits on|off, admin allow|deny, drop"
            .into(),
    }
}
