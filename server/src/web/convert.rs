// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! From what meshctl knows to what the API says. Kept apart from the core's
//! own types so that either can change without the other having to.

use crate::radios::Change;
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
        position: node.position.map(|p| api::Position {
            latitude: p.latitude,
            longitude: p.longitude,
            altitude: p.altitude,
        }),
        last_heard: node.last_heard,
        snr: node.snr,
        rssi: node.rssi,
        hops_away: node.hops_away,
        via_mqtt: node.via_mqtt,
        favorite: node.favorite,
        battery_level: node.battery_level,
        voltage: node.voltage,
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
        Change::Mesh(radio, mesh::Event::Node(n)) => ServerMessage::Node {
            radio,
            node: node(&n),
        },
        // Clients hear of these once kept, as the ones below.
        Change::Mesh(
            _,
            mesh::Event::Message(_)
            | mesh::Event::Ack(_)
            | mesh::Event::Traceroute(_)
            | mesh::Event::AdminAnswer { .. },
        ) => return None,
        Change::Traceroute(radio, traceroute) => ServerMessage::Traceroute { radio, traceroute },
        Change::Message(radio, message) => ServerMessage::Message { radio, message },
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
