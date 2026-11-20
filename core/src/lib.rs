// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl knows about radios and the mesh, independent of how it is
//! served.
//!
//! - [`link`]: a connection to a Meshtastic radio over TCP or serial. A
//!   tasticsplitter speaks the same client API as a radio, so pointing meshctl
//!   at one is pointing it at a TCP radio.
//! - [`mesh`]: the state of the radio and the mesh, and how messages change it.
//! - [`radio`]: the task that keeps the link up and the state current.

pub mod link;
pub mod mesh;
pub mod radio;
