// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::{Framing, Link};
use meshctl_proto::frame;
use std::io;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_serial::SerialPortBuilderExt;

pub async fn connect(path: &str, baud: u32, framing: Framing) -> io::Result<Link> {
    let mut port = tokio_serial::new(path, baud).open_native_async()?;

    if framing == Framing::Meshtastic {
        // Like the official clients: a run of START2 wakes a sleeping device
        // and gets the firmware out of the middle of a half-read frame.
        port.write_all(&[frame::START2; 32]).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(super::spawn(port, "serial", framing))
}
