// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use super::Link;
use std::io;
use tokio::net::TcpStream;

pub async fn connect(host: &str, port: u16) -> io::Result<Link> {
    let stream = TcpStream::connect((host, port)).await?;
    // Frames are small and a chat message should not wait for company.
    stream.set_nodelay(true)?;
    Ok(super::spawn(stream, "tcp"))
}
