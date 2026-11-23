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
pub mod convert;
mod events;
mod links;
mod meshcore;
mod messages;
mod packets;
mod preferences;
mod radios;
mod requests;
mod secrets;
mod settings;
mod storage;
mod traceroutes;

pub use auth::Auth;

use crate::radios::Radios;
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use meshctl_api::API_V1;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::Scalar;

/// Where the reference is served.
const DOCS_PATH: &str = "/api/docs";

/// Where the document behind it is served, for clients and code generators.
const OPENAPI_PATH: &str = "/api/openapi.json";

/// Scalar's page, pinned so that it does not change under us from one day to
/// the next.
const SCALAR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <title>meshctl API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
  </head>
  <body>
    <script id="api-reference" type="application/json">$spec</script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.76.4"></script>
  </body>
</html>
"#;

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
        (name = "traceroutes", description = "The routes from each radio to other nodes"),
        (name = "settings", description = "Each radio's own settings"),
        (name = "secrets", description = "What a radio keeps secret, handed out only when asked for"),
        (name = "packets", description = "What each radio hears, as it hears it"),
        (name = "requests", description = "What nodes are asked for, and answer"),
        (name = "preferences", description = "How the interface looks, for every browser"),
        (name = "storage", description = "What meshctl keeps, and for how long"),
        (name = "events", description = "What changes, as it changes"),
    ),
    components(schemas(
        meshctl_api::Error,
        meshctl_api::ErrorCode,
        meshctl_api::events::Hello,
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
        .merge(messages::router())
        .merge(links::router())
        .merge(meshcore::router())
        .merge(traceroutes::router())
        .merge(settings::router())
        .merge(secrets::router())
        .merge(preferences::router())
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
    let docs = Scalar::new(reference).custom_html(SCALAR_HTML).to_html();

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
