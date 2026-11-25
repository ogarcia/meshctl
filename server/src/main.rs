// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use anyhow::Result;
use clap::Parser;
use meshctld::Config;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

/// A lean control panel for your mesh radios.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Where to keep what meshctl knows: the radios it was given and what they
    /// heard. Created if it is not there.
    #[arg(long, env = "MESHCTL_DATA", default_value = "data")]
    data: PathBuf,

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
    let config = Config {
        data: cli.data,
        listen: cli.listen,
        token: cli.token,
        host: None,
    };
    meshctld::run_until(config, shutdown()).await
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
}
