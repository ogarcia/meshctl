// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface: whatever trunk built into `web/dist`, compiled into the
//! binary in a release build and read from disk in a debug one.

use axum::Json;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
// So that the daemon builds, and its tests run, without trunk having run.
#[allow_missing = true]
struct Interface;

/// The page every address the interface routes itself leads to.
const INDEX: &str = "index.html";

/// Answers anything no other route did.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // An API address nothing answers is an API error, not a page.
    if path == "api" || path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response();
    }

    if let Some(response) = file(path) {
        return response;
    }

    // An address without an extension is one of the interface's own, which
    // it routes once loaded. One with an extension is a file that is not there.
    let is_page = !path.rsplit('/').next().unwrap_or_default().contains('.');
    if is_page && let Some(response) = file(INDEX) {
        return response;
    }

    if Interface::get(INDEX).is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "This meshctl was built without its interface: run `trunk build` in web/ \
             before building the server. The API is at /api/v1 and its reference at /api/docs.\n",
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

fn file(path: &str) -> Option<Response> {
    let path = if path.is_empty() { INDEX } else { path };
    let file = Interface::get(path)?;
    let mime = file.metadata.mimetype().to_owned();
    // The bundle's files carry a hash in their names and can be kept forever;
    // the page that names them has to be asked for again every time.
    let cache = if path == INDEX {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, cache.to_owned()),
            ],
            file.data,
        )
            .into_response(),
    )
}
