// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's clients, each on a TCP link of its own, its frames framed as
//! the firmware frames them.

use crate::commands::answer;
use crate::radio::{Event, Radio};
use meshctl_core::meshcore::protocol::{FROM_CLIENT, FROM_RADIO};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;

pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut framed = vec![FROM_RADIO];
    framed.extend(u16::try_from(payload.len()).unwrap_or(0).to_le_bytes());
    framed.extend(payload);
    framed
}

/// One client, from connecting until it goes.
pub async fn serve(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    events: broadcast::Sender<Event>,
) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let mut pushed = events.subscribe();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        tokio::select! {
            read = reader.read(&mut chunk) => {
                let n = read?;
                if n == 0 {
                    return Ok(());
                }
                buffer.extend_from_slice(&chunk[..n]);
                // Every whole command in what came, each answered in turn.
                loop {
                    let Some(start) = buffer.iter().position(|&b| b == FROM_CLIENT) else {
                        buffer.clear();
                        break;
                    };
                    buffer.drain(..start);
                    if buffer.len() < 3 {
                        break;
                    }
                    let len = usize::from(u16::from_le_bytes([buffer[1], buffer[2]]));
                    if buffer.len() < 3 + len {
                        break;
                    }
                    let command: Vec<u8> = buffer.drain(..3 + len).skip(3).collect();
                    for answer in answer(&command, &radio, &events) {
                        writer.write_all(&frame(&answer)).await?;
                    }
                }
            }
            event = pushed.recv() => match event {
                Ok(Event::Push(payload)) => writer.write_all(&frame(&payload)).await?,
                Ok(Event::Drop) => return Ok(()),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
        }
    }
}
