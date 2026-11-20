// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Stream framing of the Meshtastic client API, the same over serial and TCP
//! and in both directions: START1, START2, the payload length as a big endian
//! u16, then a `ToRadio` or `FromRadio` protobuf.

use bytes::{Buf, BytesMut};

pub const START1: u8 = 0x94;
pub const START2: u8 = 0xC3;
pub const HEADER_SIZE: usize = 4;
/// The firmware rejects anything longer; a header announcing more is garbage
/// that happened to contain START1 START2.
pub const MAX_PAYLOAD: usize = 512;

/// Frames a payload, or `None` if it is longer than the firmware accepts.
pub fn encode(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let len = u16::try_from(payload.len()).ok()?;
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&[START1, START2]);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    Some(buf)
}

/// Splits a byte stream into frame payloads. Bytes outside a frame are dropped:
/// on serial the firmware prints its debug log in plain text between frames,
/// and clients send a run of START2 to wake the device.
#[derive(Debug, Default)]
pub struct FrameParser {
    buffer: BytesMut,
}

impl FrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in whatever was read and returns every frame it completed.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();

        loop {
            // Resynchronize on START1.
            match self.buffer.iter().position(|&b| b == START1) {
                Some(i) => self.buffer.advance(i),
                None => {
                    self.buffer.clear();
                    break;
                }
            }
            if self.buffer.len() < HEADER_SIZE {
                break;
            }
            if self.buffer[1] != START2 {
                self.buffer.advance(1);
                continue;
            }
            let len = usize::from(u16::from_be_bytes([self.buffer[2], self.buffer[3]]));
            if len > MAX_PAYLOAD {
                tracing::trace!(len, "oversized frame header, resynchronizing");
                self.buffer.advance(1);
                continue;
            }
            if self.buffer.len() < HEADER_SIZE + len {
                break;
            }
            self.buffer.advance(HEADER_SIZE);
            frames.push(self.buffer.split_to(len).to_vec());
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payload: &[u8]) -> Vec<u8> {
        encode(payload).expect("the payload fits")
    }

    #[test]
    fn roundtrip() {
        let payload = vec![0x01, 0x02, 0x03];
        let mut parser = FrameParser::new();
        assert_eq!(parser.feed(&framed(&payload)), vec![payload]);
    }

    #[test]
    fn partial_feed() {
        let payload = vec![7u8; 300];
        let encoded = framed(&payload);
        let mut parser = FrameParser::new();
        assert!(parser.feed(&encoded[..1]).is_empty());
        assert!(parser.feed(&encoded[1..10]).is_empty());
        assert_eq!(parser.feed(&encoded[10..]), vec![payload]);
    }

    #[test]
    fn skips_debug_text_and_wake_bytes() {
        let mut stream = b"INFO | booting\r\n".to_vec();
        stream.extend_from_slice(&[START2; 32]);
        stream.extend_from_slice(&framed(&[1]));
        stream.extend_from_slice(b"DEBUG | \x94 stray\n");
        stream.extend_from_slice(&framed(&[2, 3]));
        let mut parser = FrameParser::new();
        assert_eq!(parser.feed(&stream), vec![vec![1], vec![2, 3]]);
    }

    #[test]
    fn oversized_header_is_garbage() {
        let mut stream = vec![START1, START2, 0xFF, 0xFF];
        stream.extend_from_slice(&framed(&[9]));
        let mut parser = FrameParser::new();
        assert_eq!(parser.feed(&stream), vec![vec![9]]);
    }

    #[test]
    fn refuses_to_frame_what_the_firmware_would_reject() {
        assert!(encode(&[0; MAX_PAYLOAD]).is_some());
        assert!(encode(&[0; MAX_PAYLOAD + 1]).is_none());
    }
}
