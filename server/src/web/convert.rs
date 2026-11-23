// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! From what meshctl knows to what the API says. Kept apart from the core's
//! own types so that either can change without the other having to.

use crate::radios::Change;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use meshctl_api as api;
use meshctl_core::mesh;

pub fn link(state: mesh::LinkState) -> api::LinkState {
    match state {
        mesh::LinkState::Disconnected => api::LinkState::Disconnected,
        mesh::LinkState::Connecting => api::LinkState::Connecting,
        mesh::LinkState::Syncing => api::LinkState::Syncing,
        mesh::LinkState::Ready => api::LinkState::Ready,
    }
}

pub fn local(local: &mesh::LocalNode) -> api::LocalNode {
    api::LocalNode {
        num: local.num,
        id: format!("!{:08x}", local.num),
        firmware_version: local.firmware_version.clone(),
        hw_model: local.hw_model.clone(),
        role: local.role.clone(),
        region: local.region.clone(),
        modem_preset: local.modem_preset.clone(),
    }
}

pub fn channel(channel: &mesh::Channel) -> api::Channel {
    api::Channel {
        index: channel.index,
        name: channel.name.clone(),
        role: channel.role.clone(),
    }
}

pub fn node(node: &mesh::Node) -> api::Node {
    let user = node.user.as_ref();
    api::Node {
        num: node.num,
        id: node.id(),
        long_name: user.map(|u| u.long_name.clone()),
        short_name: user.map(|u| u.short_name.clone()),
        hw_model: user.map(|u| u.hw_model.clone()),
        role: user.map(|u| u.role.clone()),
        public_key: user
            .map(|u| &u.public_key)
            .filter(|key| !key.is_empty())
            .map(|key| BASE64.encode(key)),
        licensed: user.is_some_and(|u| u.licensed),
        unmessagable: user.is_some_and(|u| u.unmessagable),
        position: node.position.map(|p| api::Position {
            latitude: p.latitude,
            longitude: p.longitude,
            altitude: p.altitude,
            radius: p.radius,
            source: p.source.map(|source| match source {
                mesh::PositionSource::Manual => api::PositionSource::Manual,
                mesh::PositionSource::Internal => api::PositionSource::Internal,
                mesh::PositionSource::External => api::PositionSource::External,
            }),
        }),
        last_heard: node.last_heard,
        snr: node.snr,
        rssi: node.rssi,
        hops_away: node.hops_away,
        via_mqtt: node.via_mqtt,
        favorite: node.favorite,
        battery_level: node.battery_level,
        voltage: node.voltage,
        channel_utilization: node.channel_utilization,
        on_radio: node.on_radio,
        status: node.status.clone(),
        status_at: node.status_at,
    }
}

/// A change, as the event stream carries it.
pub fn change(change: Change) -> Option<api::events::ServerMessage> {
    use api::events::ServerMessage;
    Some(match change {
        Change::Radios => ServerMessage::Radios,
        Change::Lagged => ServerMessage::Lagged,
        Change::Mesh(radio, mesh::Event::Link(state)) => ServerMessage::Link {
            radio,
            link: link(state),
        },
        Change::Mesh(radio, mesh::Event::Synced) => ServerMessage::Synced { radio },
        Change::Mesh(radio, mesh::Event::NodeRemoved(node)) => {
            ServerMessage::NodeRemoved { radio, node }
        }
        Change::NodeForgotten(radio, node) => ServerMessage::NodeForgotten { radio, node },
        Change::MessagesForgotten(radio, ids) => ServerMessage::MessagesForgotten { radio, ids },
        Change::Cleaned(cleaned) => ServerMessage::Cleaned { cleaned },
        Change::Retention(retention) => ServerMessage::Retention { retention },
        Change::ConversationRead { radio, peer, up_to } => {
            ServerMessage::ConversationRead { radio, peer, up_to }
        }
        Change::Mesh(radio, mesh::Event::Node(n)) => ServerMessage::Node {
            radio,
            node: node(&n),
        },
        // Clients hear of these once kept, as the ones below.
        Change::Mesh(
            _,
            mesh::Event::Message(_)
            | mesh::Event::Packet(_)
            | mesh::Event::Ack(_)
            | mesh::Event::Traceroute(_)
            | mesh::Event::AdminAnswer { .. }
            | mesh::Event::Reply(_),
        ) => return None,
        Change::Traceroute(radio, traceroute) => ServerMessage::Traceroute { radio, traceroute },
        Change::Message(radio, message) => ServerMessage::Message { radio, message },
        Change::Packet(radio, packet) => ServerMessage::Packet { radio, packet },
        Change::Request(radio, request) => ServerMessage::Request { radio, request },
        Change::Preferences(preferences) => ServerMessage::Preferences { preferences },
        Change::MessageStatus {
            radio,
            id,
            status,
            error,
        } => ServerMessage::MessageStatus {
            radio,
            id,
            status,
            error,
        },
    })
}
