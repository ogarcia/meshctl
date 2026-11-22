// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Packets as clients see them: what each one carries, read through the
//! protobufs' own descriptions, so that every port the protobufs know is read
//! the same way, without code for each.

use crate::store::PacketRow;
use meshctl_api::{Packet, PacketKind};
use meshctl_proto::meshtastic::PortNum;
use prost_reflect::{DynamicMessage, SerializeOptions};

/// A kept packet, as the API tells it.
pub fn packet(row: PacketRow) -> Packet {
    let u32_of = |value: i64| u32::try_from(value).unwrap_or_default();
    let hop_limit = u32_of(row.hop_limit);
    let hop_start = u32_of(row.hop_start);
    Packet {
        id: row.id,
        packet_id: u32_of(row.packet_id),
        from: u32_of(row.from_num),
        to: u32_of(row.to_num),
        channel: u32_of(row.channel),
        port: row.portnum.map(port_name),
        payload: row.portnum.and_then(|portnum| read(portnum, &row.payload)),
        request_id: row.request_id.map(u32_of),
        reply_id: row.reply_id.map(u32_of),
        want_response: row.want_response,
        pki: row.pki,
        at: row.at,
        snr: row.snr,
        rssi: row.rssi,
        hops_away: (!row.via_mqtt && hop_start > 0).then(|| hop_start.saturating_sub(hop_limit)),
        hop_limit,
        hop_start,
        relay_node: row.relay_node.map(u32_of),
        via_mqtt: row.via_mqtt,
    }
}

/// A port's name, as the protobufs give it, or its number when they do not
/// know it.
fn port_name(portnum: i32) -> String {
    PortNum::try_from(portnum).map_or_else(|_| portnum.to_string(), |p| p.as_str_name().into())
}

/// The ports' numbers of a kind of packet; for the others, those of every
/// kind that has its own, which they are not.
pub fn portnums(kind: PacketKind) -> Vec<i32> {
    let kinds = if kind == PacketKind::Other {
        PacketKind::ALL.to_vec()
    } else {
        vec![kind]
    };
    kinds
        .into_iter()
        .flat_map(PacketKind::ports)
        .filter_map(|name| PortNum::from_str_name(name).map(|port| port as i32))
        .collect()
}

/// What a payload says, if it can be read and may be shown.
pub fn read(portnum: i32, payload: &[u8]) -> Option<serde_json::Value> {
    let message = match PortNum::try_from(portnum).ok()? {
        PortNum::TextMessageApp
        | PortNum::RangeTestApp
        | PortNum::DetectionSensorApp
        | PortNum::AlertApp => {
            return Some(String::from_utf8_lossy(payload).into_owned().into());
        }
        PortNum::PositionApp => "meshtastic.Position",
        PortNum::NodeinfoApp => "meshtastic.User",
        PortNum::RoutingApp => "meshtastic.Routing",
        PortNum::WaypointApp => "meshtastic.Waypoint",
        PortNum::TelemetryApp => "meshtastic.Telemetry",
        PortNum::TracerouteApp => "meshtastic.RouteDiscovery",
        PortNum::NeighborinfoApp => "meshtastic.NeighborInfo",
        PortNum::StoreForwardApp => "meshtastic.StoreAndForward",
        PortNum::PaxcounterApp => "meshtastic.Paxcount",
        PortNum::MapReportApp => "meshtastic.MapReport",
        // Administration carries passwords and keys, and stays unread; the
        // rest is opaque, or not worth reading here.
        _ => return None,
    };
    let descriptor = meshctl_proto::descriptors().get_message_by_name(message)?;
    let decoded = DynamicMessage::decode(descriptor, payload).ok()?;
    let options = SerializeOptions::new().use_proto_field_name(true);
    decoded
        .serialize_with_options(serde_json::value::Serializer, &options)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_proto::meshtastic as pb;
    use prost::Message;

    fn row(portnum: Option<i32>, payload: Vec<u8>) -> PacketRow {
        PacketRow {
            id: 1,
            packet_id: 42,
            from_num: 0x5568_b843,
            to_num: 0xffff_ffff,
            channel: 0,
            portnum,
            payload,
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: 1000,
            snr: Some(11.5),
            rssi: Some(-30),
            hop_limit: 3,
            hop_start: 5,
            relay_node: Some(0x43),
            via_mqtt: false,
        }
    }

    #[test]
    fn a_text_reads_as_text() {
        let packet = packet(row(Some(1), b"hola".to_vec()));
        assert_eq!(packet.port.as_deref(), Some("TEXT_MESSAGE_APP"));
        assert_eq!(packet.payload, Some("hola".into()));
        assert_eq!(packet.hops_away, Some(2));
    }

    #[test]
    fn a_protobuf_reads_as_its_json() {
        let position = pb::Position {
            latitude_i: Some(433_000_000),
            longitude_i: Some(-84_000_000),
            ..Default::default()
        };
        let packet = packet(row(Some(3), position.encode_to_vec()));
        let payload = packet.payload.unwrap();
        assert_eq!(payload["latitude_i"], 433_000_000);
        assert_eq!(payload["longitude_i"], -84_000_000);
    }

    #[test]
    fn what_is_encrypted_or_private_stays_unread() {
        let encrypted = packet(row(None, vec![1, 2, 3]));
        assert_eq!(encrypted.port, None);
        assert_eq!(encrypted.payload, None);

        let admin = packet(row(Some(6), vec![1, 2, 3]));
        assert_eq!(admin.port.as_deref(), Some("ADMIN_APP"));
        assert_eq!(admin.payload, None);

        let unknown = packet(row(Some(4000), Vec::new()));
        assert_eq!(unknown.port.as_deref(), Some("4000"));
    }
}
