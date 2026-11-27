// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The event stream, a WebSocket at `/api/v1/events`, one for every radio.
//!
//! A browser cannot put headers on a WebSocket, so the token travels in the
//! first message instead, and every client sends one, token or not:
//!
//! 1. The client sends a [`Hello`].
//! 2. The daemon answers [`ServerMessage::Ready`], or
//!    [`ServerMessage::Error`] and closes with code [`CLOSE_UNAUTHORIZED`].
//! 3. From then on the daemon sends whatever changes. Read the state through
//!    the API after `ready`, not before, and nothing is missed in between.
//!
//! [`ServerMessage::Synced`] means what was read of that radio is out of date;
//! [`ServerMessage::Lagged`], that everything is. Either way, read it again.

use crate::{
    ErrorCode, LinkState, Message, MessageId, MessageStatus, Node, Notification, Packet, Peer,
    Preferences, RadioId, Request,
};
use serde::{Deserialize, Serialize};

/// Where the event stream is served, under [`crate::API_V1`].
pub const EVENTS_PATH: &str = "/events";

/// The close code for a client that did not give the right token.
pub const CLOSE_UNAUTHORIZED: u16 = 4401;

/// The first message a client sends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hello {
    /// The access token, when the daemon asks for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// What a client says after its hello, whenever it changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// The conversation it shows, and is seen: nothing in it is notified of
    /// while it does. None, if it shows none, or is not seen.
    Viewing {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        viewing: Option<Viewing>,
    },
}

/// A conversation shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Viewing {
    pub radio: RadioId,
    pub peer: Peer,
}

/// Everything the daemon sends on the event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The client is in; events follow.
    Ready,
    /// Which interface the daemon serves, right after `ready`: a hash of its
    /// page, which changes with every build of it. A page that heard another
    /// before runs an interface the daemon no longer serves, and should be
    /// reloaded. Not sent by a daemon built without its interface.
    Interface { build: String },
    /// The client is not in, and the stream closes.
    Error { error: ErrorCode },
    /// The client fell behind and missed some events.
    Lagged,
    /// A radio was added, changed or removed.
    Radios,
    /// The link to a radio changed.
    Link { radio: RadioId, link: LinkState },
    /// A radio sent its whole configuration and node list again.
    Synced { radio: RadioId },
    /// A radio heard a node, or something about one changed.
    Node { radio: RadioId, node: Node },
    /// A radio forgot a node, as asked: out of its node list.
    NodeRemoved { radio: RadioId, node: u32 },
    /// All kept of a node was forgotten, as asked: what was asked of it, the
    /// routes traced to it, its conversation and what it sent.
    NodeForgotten { radio: RadioId, node: u32 },
    /// A message heard or sent through a radio.
    Message { radio: RadioId, message: Message },
    /// A waypoint heard or shared through a radio, new or changed.
    Waypoint {
        radio: RadioId,
        waypoint: crate::waypoints::Waypoint,
    },
    /// A waypoint gone from a radio: deleted by whoever may, expired, or
    /// forgotten in meshctl.
    WaypointGone { radio: RadioId, id: u32 },
    /// A node gone in or out of a fence meshctl watches, whether or not
    /// notifications are on: a client open tells of it its own way.
    FenceCrossed {
        radio: RadioId,
        waypoint: u32,
        crossing: crate::waypoints::FenceCrossing,
        /// Whether its fence asks to be told of it: going in, or out, of
        /// the favourites only, maybe. Kept all the same if not.
        asked: bool,
        /// Who crossed it, by name, and the waypoint's emoji and name: to
        /// tell of it with no more to read.
        who: String,
        icon: String,
        name: String,
    },
    /// A conversation read further, in any client: up to which message.
    ConversationRead {
        radio: RadioId,
        peer: Peer,
        up_to: MessageId,
    },
    /// A conversation silenced, or no longer, in any client.
    ConversationMuted {
        radio: RadioId,
        peer: Peer,
        muted: bool,
    },
    /// A message to notify of, where notifications are let be shown: the
    /// app meshctl runs within shows it on its own.
    Notification { notification: Notification },
    /// Messages forgotten, as asked of the daemon: one, and the reactions to
    /// it.
    MessagesForgotten { radio: RadioId, ids: Vec<MessageId> },
    /// A conversation's messages forgotten, as asked of the daemon: a
    /// channel emptied, which stays on the radio, or a conversation with a
    /// node gone.
    ConversationForgotten { radio: RadioId, peer: Peer },
    /// What was kept for longer than it is kept, or as asked, cleaned away:
    /// whatever shows what was kept is to be read again.
    Cleaned { cleaned: crate::Cleaned },
    /// How long what meshctl keeps is kept changed.
    Retention { retention: crate::Retention },
    /// What became of a message sent.
    MessageStatus {
        radio: RadioId,
        id: MessageId,
        status: MessageStatus,
        error: Option<String>,
    },
    /// What the console of a MeshCore node answered a radio logged in to it as
    /// its admin, as each comes: when, by the radio's clock in seconds.
    Console {
        radio: RadioId,
        node: u32,
        text: String,
        at: i64,
    },
    /// A login to a MeshCore node sent through a radio, and how long, in
    /// milliseconds from now, its answer is waited for: the login's request
    /// answers when it comes, or when that runs out.
    LoginWaiting {
        radio: RadioId,
        node: u32,
        wait_ms: u32,
    },
    /// Another copy of a message heard, through a MeshCore radio: how many
    /// times it was heard now, and the region it came in.
    MessageHeard {
        radio: RadioId,
        id: MessageId,
        heard: u32,
        /// How many repeaters it was heard through.
        #[serde(default)]
        repeaters: u32,
        scope: Option<String>,
    },
    /// Something a node was asked for through a radio, or what became of it.
    Request { radio: RadioId, request: Request },
    /// A packet a radio heard.
    Packet { radio: RadioId, packet: Packet },
    /// The interface's preferences changed, maybe from another browser.
    Preferences { preferences: Preferences },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_say_their_type_and_radio() {
        let message = ServerMessage::Link {
            radio: 1,
            link: LinkState::Ready,
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"type":"link","radio":1,"link":"ready"}"#
        );
    }

    #[test]
    fn a_hello_may_be_empty() {
        assert_eq!(
            serde_json::from_str::<Hello>("{}").unwrap(),
            Hello::default()
        );
    }
}
