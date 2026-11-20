// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A connection to a radio, carrying frame payloads: `FromRadio` protobufs in,
//! `ToRadio` protobufs out. What they mean is somebody else's business.

mod serial;
mod tcp;

use meshctl_proto::frame;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The port a Meshtastic radio serves its client API on over the network.
pub const DEFAULT_TCP_PORT: u16 = 4403;

/// The speed the Meshtastic firmware talks at over USB serial.
pub const DEFAULT_BAUD: u32 = 115_200;

/// Where the radio is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// A radio on the network, or a tasticsplitter in front of one.
    Tcp { host: String, port: u16 },
    /// A radio plugged in, by the path of its serial device.
    Serial { path: String, baud: u32 },
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp { host, port } => write!(f, "tcp://{host}:{port}"),
            Self::Serial { path, baud } => write!(f, "serial://{path}@{baud}"),
        }
    }
}

/// A connected radio. Payloads arrive on `rx`, which closes when the link
/// drops, and leave through `tx`. Dropping the link closes the connection.
pub struct Link {
    pub tx: mpsc::UnboundedSender<Vec<u8>>,
    pub rx: mpsc::UnboundedReceiver<Vec<u8>>,
    task: JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Connects to the radio.
pub async fn connect(transport: &Transport) -> io::Result<Link> {
    match transport {
        Transport::Tcp { host, port } => tcp::connect(host, *port).await,
        Transport::Serial { path, baud } => serial::connect(path, *baud).await,
    }
}

/// Runs both halves of a byte stream until either fails.
fn spawn<S>(stream: S, name: &'static str) -> Link
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (tx, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let (incoming, rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let task = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(stream);

        let read = async move {
            let mut buf = vec![0u8; 4096];
            let mut parser = frame::FrameParser::new();
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => {
                        tracing::warn!("{name}: connection closed");
                        break;
                    }
                    Ok(n) => {
                        for payload in parser.feed(&buf[..n]) {
                            tracing::trace!(len = payload.len(), "{name}: received frame");
                            if incoming.send(payload).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "{name}: read error");
                        break;
                    }
                }
            }
        };

        let write = async move {
            while let Some(payload) = outgoing.recv().await {
                let Some(framed) = frame::encode(&payload) else {
                    tracing::error!(len = payload.len(), "{name}: payload too long, dropped");
                    continue;
                };
                if let Err(error) = writer.write_all(&framed).await {
                    tracing::error!(%error, "{name}: write error");
                    break;
                }
                if let Err(error) = writer.flush().await {
                    tracing::error!(%error, "{name}: flush error");
                    break;
                }
                tracing::trace!(len = payload.len(), "{name}: sent frame");
            }
        };

        // Whichever half ends first takes the link down; dropping the other
        // closes `incoming`, which is how the owner sees the disconnect.
        tokio::select! {
            () = read => {},
            () = write => {},
        }
    });

    Link { tx, rx, task }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn payloads_cross_in_both_directions() {
        let (ours, mut radio) = tokio::io::duplex(1024);
        let mut link = spawn(ours, "test");

        radio
            .write_all(&frame::encode(b"from radio").unwrap())
            .await
            .unwrap();
        assert_eq!(link.rx.recv().await.unwrap(), b"from radio");

        link.tx.send(b"to radio".to_vec()).unwrap();
        let mut buf = vec![0u8; frame::HEADER_SIZE + 8];
        radio.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, frame::encode(b"to radio").unwrap());
    }

    #[tokio::test]
    async fn the_link_closes_when_the_radio_goes() {
        let (ours, radio) = tokio::io::duplex(1024);
        let mut link = spawn(ours, "test");
        drop(radio);
        assert!(link.rx.recv().await.is_none());
    }
}
