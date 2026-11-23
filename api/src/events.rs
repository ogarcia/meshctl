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
    ErrorCode, LinkState, Message, MessageId, MessageStatus, Node, Packet, Peer, Preferences,
    RadioId, Request, Traceroute,
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
    /// A conversation read further, in any client: up to which message.
    ConversationRead {
        radio: RadioId,
        peer: Peer,
        up_to: MessageId,
    },
    /// Messages forgotten, as asked of the daemon: one, and the reactions to
    /// it.
    MessagesForgotten { radio: RadioId, ids: Vec<MessageId> },
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
    /// A traceroute sent through a radio, or what became of it.
    Traceroute {
        radio: RadioId,
        traceroute: Traceroute,
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
