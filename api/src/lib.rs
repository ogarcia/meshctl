// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What travels between the daemon and its clients: the web interface today,
//! a CLI or a TUI tomorrow. Every client reads the same types the daemon
//! writes, so a change here is a compile error wherever it matters.
//!
//! Nothing here is text for people. Errors and states travel as codes, and each
//! client puts them in its own words and language.

use serde::{Deserialize, Serialize};

/// Where version 1 of the API is served.
pub const API_V1: &str = "/api/v1";

/// The header that carries the access token, when the daemon is given one.
///
/// Not `Authorization`, so that it can sit behind a proxy that uses that one
/// for its own authentication (basic auth in nginx, say) without the two
/// stepping on each other.
pub const TOKEN_HEADER: &str = "x-meshctl-token";

/// What is answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Version {
    /// The program's name.
    #[cfg_attr(feature = "openapi", schema(example = "meshctl"))]
    pub name: String,
    /// Its version, as Cargo writes it.
    #[cfg_attr(feature = "openapi", schema(example = "0.1.0"))]
    pub version: String,
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The access token is missing or wrong.
    Unauthorized,
    /// There is nothing at that address.
    NotFound,
}

/// The body of every error the API answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Error {
    pub error: ErrorCode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_travel_as_codes() {
        let body = Error {
            error: ErrorCode::NotFound,
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"not_found"}"#
        );
    }
}
