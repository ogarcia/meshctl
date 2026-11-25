// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl's daemon, as a library: what the `meshctl` binary runs, and what
//! a desktop shell can run within itself, serving the panel to its own
//! window.

mod auto_replies;
mod channels;
mod keys;
mod links;
mod lora;
mod meshcore;
mod notifications;
mod packets;
mod radios;
mod reactions;
mod routes;
mod sections;
mod settings;
mod store;
mod web;

use anyhow::{Context, Result};
use radios::Radios;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use store::Store;
use tracing::{info, warn};

/// The app the daemon runs within, if any, and what it lets be set.
pub trait Host: Send + Sync + 'static {
    /// Whether the app goes on running, and the daemon in it, with its
    /// window closed: told as it starts, as it was left, and as it changes.
    fn keep_running(&self, on: bool);

    /// Shows a notification of a message, its conversation's, in place of
    /// one shown before for it. Nothing, where the app shows none.
    fn notify(&self, _notification: &meshctl_api::Notification) {}

    /// A conversation read: its notification, if one is shown, goes.
    fn read(&self, _radio: meshctl_api::RadioId, _peer: meshctl_api::Peer) {}
}

/// How the daemon runs.
#[derive(Clone)]
pub struct Config {
    /// Where to keep what meshctl knows: the radios it was given and what
    /// they heard. Created if it is not there.
    pub data: PathBuf,
    /// Where to serve the panel and the API.
    pub listen: SocketAddr,
    /// An access token the API asks for in the X-Meshctl-Token header; none,
    /// or an empty one, leaves the API open to whoever reaches it.
    pub token: Option<String>,
    /// The app it runs within, if one asks to be told what it lets be set.
    pub host: Option<std::sync::Arc<dyn Host>>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data", &self.data)
            .field("listen", &self.listen)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .field("host", &self.host.is_some())
            .finish()
    }
}

/// Runs the daemon: opens what it keeps, connects to its radios and serves
/// the panel and the API, until `shutdown` resolves; then lets the radios
/// go and closes what it keeps, cleanly.
pub async fn run_until(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let token = config.token.filter(|token| !token.is_empty());

    info!(
        "{} {} — free software under the GPL, version 3 or later",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    );

    if !config.listen.ip().is_loopback() && token.is_none() {
        warn!(
            "listening on {} with no access token: anyone who reaches it can use the radios. \
             Put a proxy with authentication in front, or set MESHCTL_TOKEN",
            config.listen
        );
    }

    let store = Store::open(&config.data).await?;
    let radios = Radios::start(store.clone()).await?;
    if let Some(host) = config.host {
        radios.set_host(host).await;
    }
    match radios.list().await?.len() {
        0 => info!("no radios yet: add one from the panel"),
        n => info!(radios = n, "radios loaded"),
    }

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.listen))?;
    info!("serving on http://{}", config.listen);

    let state = web::AppState {
        auth: web::Auth::new(token),
        radios: radios.clone(),
    };
    let stopping = radios.clone();
    axum::serve(listener, web::router(state))
        .with_graceful_shutdown(async move {
            shutdown.await;
            info!("stopping");
            // Ends the event streams too, which would otherwise keep the
            // server waiting for them forever.
            stopping.stop().await;
        })
        .await?;

    store.close().await;
    info!("stopped");
    Ok(())
}
