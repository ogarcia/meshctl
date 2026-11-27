// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! HTTP: the API, its reference, and the interface.
//!
//! The API lives under `/api/v1`, and every route is declared through utoipa's
//! router, so the reference at `/api/docs` is built from the same calls that
//! register the handlers and cannot drift from them.
//!
//! Reading is `GET` and changing is anything else. That is what lets a proxy
//! in front open the read-only part to everyone and keep the rest for whoever
//! it authenticates, with a rule of its own and nothing here.

mod api;
mod assets;
mod auth;
mod auto_replies;
pub mod convert;
mod devices;
mod events;
mod links;
mod meshcore;
mod messages;
mod node_settings;
mod packets;
mod preferences;
mod radios;
mod requests;
mod secrets;
mod settings;
mod storage;

pub use auth::Auth;

use crate::radios::Radios;
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use meshctl_api::API_V1;
use meshctl_api::mark;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::Scalar;

/// Where the reference is served.
const DOCS_PATH: &str = "/api/docs";

/// Where the document behind it is served, for clients and code generators.
const OPENAPI_PATH: &str = "/api/openapi.json";

/// Scalar's page, pinned so that it does not change under us from one day to
/// the next, in meshctl's colours and under a bar of its own: the way back to
/// the panel, and to the guide, the website and the code. Its icon, `$icon`,
/// is the panel's in a red no accent is, to tell the two apart among tabs and
/// bookmarks. Scalar's own tools (an SDK, a deploy, an AI to ask) have nothing
/// to do with meshctl, and go.
///
/// Scalar puts `light-mode` or `dark-mode` on the body, as the system is or
/// as its switch says; its colours are overridden on those, more precisely
/// than its own theme does, and the bar takes them too, so it follows.
const SCALAR_HTML: &str = r##"<!doctype html>
<html lang="en">
  <head>
    <title>meshctl API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="icon" type="image/svg+xml" href="$icon" />
    <style>
      :root { --scalar-custom-header-height: 53px; }
      body.light-mode {
        --scalar-background-1: #ffffff;
        --scalar-background-2: #f4f5f7;
        --scalar-background-3: #eef0f3;
        --scalar-background-accent: #9a5b0024;
        --scalar-border-color: #d8dde3;
        --scalar-color-1: #161b22;
        --scalar-color-2: #57606a;
        --scalar-color-3: #8b949e;
        --scalar-color-accent: #9a5b00;
      }
      body.dark-mode {
        --scalar-background-1: #12161c;
        --scalar-background-2: #1b222b;
        --scalar-background-3: #2a3340;
        --scalar-background-accent: #f0a63a29;
        --scalar-border-color: #2a3340;
        --scalar-color-1: #e6edf3;
        --scalar-color-2: #9aa7b4;
        --scalar-color-3: #6e7b88;
        --scalar-color-accent: #f0a63a;
      }
      .mc-bar {
        position: sticky;
        top: 0;
        z-index: 1000;
        box-sizing: border-box;
        height: 53px;
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 0 16px;
        background: var(--scalar-background-1);
        border-bottom: 1px solid var(--scalar-border-color);
        font: 14px/1 var(--scalar-font, system-ui, sans-serif);
      }
      .mc-bar a {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        height: 34px;
        padding: 0 10px;
        border-radius: 8px;
        color: var(--scalar-color-2);
        text-decoration: none;
        white-space: nowrap;
      }
      .mc-bar a:hover { background: var(--scalar-background-2); color: var(--scalar-color-1); }
      .mc-bar .brand { padding-left: 0; color: var(--scalar-color-1); font-size: 16px; font-weight: 650; }
      .mc-bar .brand:hover { background: none; }
      .mc-bar .brand svg { width: 30px; height: 30px; color: var(--scalar-color-accent); }
      .mc-bar .tag {
        padding: 3px 7px;
        border-radius: 6px;
        background: var(--scalar-background-accent);
        color: var(--scalar-color-accent);
        font-size: 12px;
        font-weight: 600;
      }
      .mc-bar .gap { flex: 1; }
      .mc-bar .out { font-size: 12px; }
      .mc-bar .panel { color: var(--scalar-color-accent); font-weight: 600; }
      @media (max-width: 640px) {
        .mc-bar { padding: 0 8px; }
        .mc-bar .wide { display: none; }
      }
    </style>
  </head>
  <body>
    <header class="mc-bar">
      <a class="brand" href="/" title="meshctl">
        <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-linecap="round" aria-hidden="true"><path stroke-width="2.2" d="M9.31 18.25L6 26M17.9 15.37L26 20M6 26L26 20M18.17 9.15L25 6"/><g fill="currentColor" stroke="none"><circle cx="6" cy="26" r="4.1"/><circle cx="26" cy="20" r="4.1"/><circle cx="25" cy="6" r="4.1"/><path fill-rule="evenodd" d="M11.07 8.01L11.11 6.67L12.89 6.67L12.93 8.01L14.17 8.52L15.14 7.61L16.39 8.86L15.48 9.83L15.99 11.07L17.33 11.11L17.33 12.89L15.99 12.93L15.48 14.17L16.39 15.14L15.14 16.39L14.17 15.48L12.93 15.99L12.89 17.33L11.11 17.33L11.07 15.99L9.83 15.48L8.86 16.39L7.61 15.14L8.52 14.17L8.01 12.93L6.67 12.89L6.67 11.11L8.01 11.07L8.52 9.83L7.61 8.86L8.86 7.61L9.83 8.52ZM13.6 12A1.6 1.6 0 1 0 10.4 12A1.6 1.6 0 1 0 13.6 12Z"/></g></svg>
        meshctl
      </a>
      <span class="tag">API</span>
      <span class="gap"></span>
      <a class="wide" href="https://meshctl.ogarcia.me/guide.html">Guide</a>
      <a class="wide" href="https://meshctl.ogarcia.me/">Website</a>
      <a href="https://github.com/ogarcia/meshctl">GitHub<span class="out" aria-hidden="true">&#x2197;&#xFE0E;</span></a>
      <a class="panel" href="/">Panel</a>
    </header>
    <script id="api-reference" type="application/json" data-configuration='{"showDeveloperTools":"never","agent":{"disabled":true}}'>$spec</script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.76.4"></script>
  </body>
</html>
"##;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "meshctl",
        description = include_str!("intro.md"),
        license(name = "GPL-3.0-or-later", identifier = "GPL-3.0-or-later"),
    ),
    tags(
        (name = "meta", description = "What is answering"),
        (name = "radios", description = "The radios meshctl keeps connected to"),
        (name = "nodes", description = "The nodes each radio hears"),
        (name = "chat", description = "What each radio hears and says"),
        (name = "settings", description = "Each radio's own settings"),
        (name = "secrets", description = "What a radio keeps secret, handed out only when asked for"),
        (name = "packets", description = "What each radio hears, as it hears it"),
        (name = "requests", description = "What nodes are asked for, and answer"),
        (name = "preferences", description = "How the interface looks, for every browser"),
        (name = "auto-replies", description = "What meshctl answers on its own, and where"),
        (name = "storage", description = "What meshctl keeps, and for how long"),
        (name = "events", description = "What changes, as it changes"),
    ),
    components(schemas(
        meshctl_api::Error,
        meshctl_api::ErrorCode,
        meshctl_api::events::Hello,
        meshctl_api::events::ClientMessage,
        meshctl_api::events::ServerMessage,
    )),
)]
struct Reference;

/// What every handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub auth: Auth,
    pub radios: Radios,
}

/// Every route meshctl answers.
pub fn router(state: AppState) -> Router {
    // The event stream checks the token itself, in its first message: a
    // browser cannot put a header on a WebSocket.
    let guarded = api::router()
        .merge(radios::router())
        .merge(devices::router())
        .merge(messages::router())
        .merge(links::router())
        .merge(meshcore::router())
        .merge(settings::router())
        .merge(node_settings::router())
        .merge(secrets::router())
        .merge(preferences::router())
        .merge(auto_replies::router())
        .merge(storage::router())
        .merge(packets::router())
        .merge(requests::router())
        .layer(axum::middleware::from_fn_with_state(
            state.auth.clone(),
            auth::check,
        ));
    let (v1, reference) = OpenApiRouter::with_openapi(Reference::openapi())
        .nest(API_V1, guarded.merge(events::router()))
        .split_for_parts();

    let spec = serde_json::to_string(&reference).expect("the reference serializes");
    let page = SCALAR_HTML.replace("$icon", &mark::data_url(&mark::api_icon()));
    let docs = Scalar::new(reference).custom_html(page).to_html();

    Router::new()
        .merge(v1)
        .route(DOCS_PATH, get(move || async move { Html(docs) }))
        .route(
            OPENAPI_PATH,
            get(move || async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    spec,
                )
            }),
        )
        .fallback(assets::serve)
        .with_state(state)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use meshctl_api::{RadioKind, RadioSettings};
    use meshctl_core::mesh::{Channel, LinkState, LocalNode, MeshState, Node, User};
    use meshctl_core::radio::Mesh;
    use serde::de::DeserializeOwned;
    use tower::ServiceExt;

    /// The radio [`app`] has, with [`mesh`] for what it knows.
    pub const RADIO: i64 = 1;

    /// A mesh as a radio would have left it: synced, two nodes.
    pub fn mesh() -> Mesh {
        let mut state = MeshState {
            link: LinkState::Ready,
            local: Some(LocalNode {
                num: 0x3134_c955,
                firmware_version: Some("2.8.2.d5f1619".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        for (num, name, heard) in [(0x3134_c955, "Home", 200), (0x90cd_a275, "Faro", 100)] {
            state.nodes.insert(
                num,
                Node {
                    num,
                    user: Some(User {
                        long_name: name.into(),
                        ..Default::default()
                    }),
                    position: None,
                    last_heard: Some(heard),
                    snr: None,
                    rssi: None,
                    hops_away: Some(0),
                    via_mqtt: false,
                    favorite: false,
                    battery_level: None,
                    voltage: None,
                    channel_utilization: None,
                    on_radio: None,
                    status: None,
                    status_at: None,
                },
            );
        }
        for (index, name, role) in [
            (0, "", "PRIMARY"),
            (4, "Moire", "SECONDARY"),
            (6, "", "DISABLED"),
        ] {
            state.channels.insert(
                index,
                Channel {
                    index,
                    name: name.into(),
                    role: role.into(),
                },
            );
        }
        Mesh::new(state)
    }

    /// The radios of [`app`]: [`RADIO`], stored disabled so that nothing tries
    /// to reach it, and [`mesh`] put in place of its link.
    pub async fn radios() -> Radios {
        let store = Store::in_memory().await;
        let record = store
            .add_radio(&RadioSettings {
                name: "Home".into(),
                kind: RadioKind::Meshtastic,
                address: "127.0.0.1:1".into(),
                enabled: false,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(record.id, RADIO);
        let radios = Radios::start(store).await.unwrap();
        radios.insert_mesh(RADIO, mesh());
        radios
    }

    pub async fn app(token: Option<&str>) -> Router {
        router(AppState {
            auth: Auth::new(token.map(str::to_owned)),
            radios: radios().await,
        })
    }

    pub fn json<T: DeserializeOwned>(body: &[u8]) -> T {
        serde_json::from_slice(body).unwrap()
    }

    /// Asks the router, the way axum would, and returns the status and body.
    pub async fn ask(router: Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, body.to_vec())
    }

    pub fn get(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn the_reference_documents_the_api() {
        let (status, body) = ask(app(None).await, get(OPENAPI_PATH)).await;
        assert_eq!(status, StatusCode::OK);
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for path in [
            "/api/v1/version",
            "/api/v1/radios",
            "/api/v1/radios/{radio}/nodes",
            "/api/v1/events",
        ] {
            assert!(spec["paths"][path]["get"].is_object(), "{path}");
        }
    }

    #[tokio::test]
    async fn the_reference_has_a_page() {
        let (status, body) = ask(app(None).await, get(DOCS_PATH)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8(body).unwrap().contains("/api/v1/version"));
    }

    #[tokio::test]
    async fn an_unknown_api_address_answers_in_json() {
        let (status, body) = ask(app(None).await, get("/api/v1/nowhere")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, br#"{"error":"not_found"}"#);
    }
}
