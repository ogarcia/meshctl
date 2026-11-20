// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

mod web;

use anyhow::{Context, Result};
use clap::Parser;
use meshctl_core::link::Transport;
use meshctl_core::radio::Mesh;
use std::net::SocketAddr;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// A lean control panel for your Meshtastic node.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Where the radio is: `host` or `host:port` (a radio on the network, or a
    /// tasticsplitter in front of one), or the path of its serial device, with
    /// `@speed` after it when it is not 115200.
    #[arg(long, env = "MESHCTL_RADIO")]
    radio: Transport,

    /// Where to serve the panel and the API. Only this machine by default:
    /// serving anywhere else is a decision to make, not a default to inherit.
    //
    // The port is one no registered service uses and few programs pick, so that
    // it does not collide with whatever else runs on a small server.
    #[arg(long, env = "MESHCTL_LISTEN", default_value = "127.0.0.1:7434")]
    listen: SocketAddr,

    /// An access token the API asks for in the X-Meshctl-Token header. Without
    /// one the API is open to whoever reaches it; put a proxy with its own
    /// authentication in front, or give it one.
    #[arg(long, env = "MESHCTL_TOKEN", hide_env_values = true)]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    let token = cli.token.filter(|token| !token.is_empty());

    info!(
        "{} {} — free software under the GPL, version 3 or later",
        env!("CARGO_BIN_NAME"),
        env!("CARGO_PKG_VERSION"),
    );

    if !cli.listen.ip().is_loopback() && token.is_none() {
        warn!(
            "listening on {} with no access token: anyone who reaches it can use the radio. \
             Put a proxy with authentication in front, or set MESHCTL_TOKEN",
            cli.listen
        );
    }

    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("cannot listen on {}", cli.listen))?;
    info!("serving on http://{}", cli.listen);

    let mesh = Mesh::default();
    let radio = mesh.spawn(cli.radio);
    let state = web::AppState {
        auth: web::Auth::new(token),
        mesh,
    };

    axum::serve(listener, web::router(state))
        .with_graceful_shutdown(shutdown())
        .await?;

    // Dropping the link closes the connection to the radio.
    radio.abort();
    let _ = radio.await;
    info!("stopped");
    Ok(())
}

/// Resolves on Ctrl+C, or on the SIGTERM a service manager or a container
/// runtime stops it with.
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    info!("stopping");
}
