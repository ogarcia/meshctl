// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface: whatever trunk built into `web/dist`, compiled into the
//! binary in a release build and read from disk in a debug one.

use super::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{Accent, Error, ErrorCode, MapTheme, Preferences, Theme};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
// So that the daemon builds, and its tests run, without trunk having run.
#[allow_missing = true]
struct Interface;

/// The page every address the interface routes itself leads to.
const INDEX: &str = "index.html";

/// Which interface this daemon serves: its page's hash, which names the
/// scripts and styles of that build by their own hashes. `None` without
/// one.
pub fn build() -> Option<String> {
    let index = Interface::get(INDEX)?;
    Some(
        index
            .metadata
            .sha256_hash()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Answers anything no other route did.
pub async fn serve(State(state): State<AppState>, uri: Uri) -> Response {
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

    // An address without an extension is one of the interface's own, which
    // it routes once loaded. One with an extension is a file that is not there.
    let is_page = !path.rsplit('/').next().unwrap_or_default().contains('.');
    if (path.is_empty() || path == INDEX || is_page)
        && let Some(index) = Interface::get(INDEX)
    {
        return page(&state, &index.data).await;
    }

    if let Some(response) = file(path) {
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

/// The page, already in the colours it is to be shown in, so that it does not
/// flash in others while the interface loads and asks for them.
async fn page(state: &AppState, html: &[u8]) -> Response {
    let preferences = state.radios.preferences().await.unwrap_or_else(|err| {
        tracing::warn!(
            error = format_args!("{err:#}"),
            "cannot read the preferences"
        );
        Preferences::default()
    });
    let html = themed(&String::from_utf8_lossy(html), &preferences);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // It names the bundle's files, which change with every build.
            (header::CACHE_CONTROL, "no-cache"),
        ],
        html,
    )
        .into_response()
}

/// The page with the preferences on its `<html>`, as the interface keeps them
/// there: no `data-theme` when it follows the system's.
fn themed(html: &str, preferences: &Preferences) -> String {
    let mut attributes = String::new();
    match preferences.theme {
        Theme::System => {}
        Theme::Light => attributes.push_str(r#" data-theme="light""#),
        Theme::Dark => attributes.push_str(r#" data-theme="dark""#),
    }
    attributes.push_str(match preferences.accent {
        Accent::Amber => r#" data-accent="amber""#,
    });
    match preferences.map_theme {
        MapTheme::Page => {}
        MapTheme::Light => attributes.push_str(r#" data-map-theme="light""#),
        MapTheme::Dark => attributes.push_str(r#" data-map-theme="dark""#),
    }
    match html.find("<html") {
        Some(at) => {
            let at = at + "<html".len();
            format!("{}{attributes}{}", &html[..at], &html[at..])
        }
        None => html.to_owned(),
    }
}

fn file(path: &str) -> Option<Response> {
    let file = Interface::get(path)?;
    let mime = file.metadata.mimetype().to_owned();
    let cache = if lasting(path) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
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

/// Whether a file of the interface never changes under its name, and can be
/// kept for good: those trunk names by their hash (`map-c1368161483d3368.js`)
/// and those in a folder that carries its version (`leaflet-1.9.4/…`). The
/// rest, as the icons, the manifest or the service worker, are asked for
/// again each time, and come back only if they changed.
fn lasting(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();
    let stem = name.split('.').next().unwrap_or_default();
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    let hashed = stem
        .rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()));
    let versioned = path.contains('/')
        && path.split('/').next().is_some_and(|folder| {
            folder.rsplit_once('-').is_some_and(|(_, version)| {
                version.contains('.') && version.split('.').all(|part| part.parse::<u32>().is_ok())
            })
        });
    hashed || versioned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_never_changes_under_its_name_is_kept_for_good() {
        assert!(lasting("map-c1368161483d3368.js"));
        assert!(lasting("meshctl-web-3581867ad566c4bb_bg.wasm"));
        assert!(lasting("leaflet-1.9.4/dist/leaflet.js"));
        assert!(lasting("tabler-icons-3.51.0/icons/outline/x.svg"));
        assert!(!lasting("favicon.svg"));
        assert!(!lasting("sw.js"));
        assert!(!lasting("pwa/manifest.webmanifest"));
        assert!(!lasting("pwa/icon-192.png"));
    }

    #[test]
    fn the_page_comes_in_its_colours() {
        let html = "<!doctype html>\n<html lang=\"en\"><body></body></html>";
        assert_eq!(
            themed(html, &Preferences::default()),
            "<!doctype html>\n<html data-accent=\"amber\" lang=\"en\"><body></body></html>"
        );
        let dark = Preferences {
            theme: Theme::Dark,
            map_theme: MapTheme::Light,
            ..Default::default()
        };
        assert!(themed(html, &dark).contains(
            r#"<html data-theme="dark" data-accent="amber" data-map-theme="light" lang="en">"#
        ));
    }
}
