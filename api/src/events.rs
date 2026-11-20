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

use crate::{ErrorCode, LinkState, Node, RadioId};
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
