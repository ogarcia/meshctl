// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What both pretend radios share: how they are started, their clients and
//! control port served, the time, and chance. Each radio is a binary of its
//! own, in `src/bin`; see the README.

use std::error::Error;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// A number from 0 up to, not including, `below`: not for secrets, only so
/// that answers come at their own time.
pub fn random(below: u64) -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x2545_f491, |d| d.as_nanos() as u64)
            | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x % below.max(1)
}

/// Now, in seconds since the epoch.
pub fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
}

/// The ports a radio is started with: the one its clients reach it on, the
/// first argument or `default`, and the control port, the second, if any.
pub fn ports(default: u16) -> Result<(u16, Option<u16>), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().map_or(Ok(default), |p| p.parse())?;
    let control = args.next().map(|p| p.parse()).transpose()?;
    Ok((port, control))
}

/// Serves a radio: its clients on `port`, on every address, each handed to
/// `serve`; and, with a control port, lines on it, on this machine only,
/// each answered by `answer`. `banner` says which radio it is, once it
/// listens.
pub async fn run<S, F, A>(
    port: u16,
    control: Option<u16>,
    banner: &str,
    serve: S,
    answer: A,
) -> Result<(), Box<dyn Error>>
where
    S: Fn(TcpStream) -> F,
    F: Future<Output = std::io::Result<()>> + Send + 'static,
    A: Fn(&str) -> String + Clone + Send + Sync + 'static,
{
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("{banner}");
    if let Some(control_port) = control {
        let control = TcpListener::bind(("127.0.0.1", control_port)).await?;
        println!("control on 127.0.0.1:{control_port}");
        tokio::spawn(async move {
            while let Ok((stream, _)) = control.accept().await {
                let answer = answer.clone();
                tokio::spawn(async move {
                    let _ = controlled(stream, answer).await;
                });
            }
        });
    }
    loop {
        let (stream, address) = listener.accept().await?;
        println!("client {address}");
        let serving = serve(stream);
        tokio::spawn(async move {
            if let Err(error) = serving.await {
                println!("client {address} gone: {error}");
            }
        });
    }
}

/// One link to the control port: each line answered, and what it did
/// printed as well.
async fn controlled(stream: TcpStream, answer: impl Fn(&str) -> String) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let said = answer(&line);
        println!("control: {line} -> {said}");
        writer.write_all(format!("{said}\n").as_bytes()).await?;
    }
    Ok(())
}
