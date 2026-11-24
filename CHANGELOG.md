# Changelog

What each release brings. Until 1.0, a release may change the interface, the
API and the database; each one carries the database forward, as far as it can.

## 0.1.0 (2026-11-24)

The first release: meshctl as it is, to be tried.

### Radios

- Meshtastic and MeshCore radios, as many as you have, over the network or a
  cable, added, ordered, hidden and changed from the panel; each with its own
  colour and short name on the rail.
- Connected around the clock, and again on their own when the link drops,
  what they hear kept in SQLite, for as long as you say.

### The mesh

- Every node heard, when, how well and how far, on a list and on a map; its
  page with what it says about itself, its position, and what it can be asked
  for: status, telemetry, neighbours, position, its owner, a sensor's readings
  over a time, a room's or a repeater's access, a ping.
- Traceroutes on both meshes, and MeshCore's route discovery, with their
  history; every hop on the map, those that are only deduced marked as such.
- The live view: every packet the radio hears, as it hears it, with the route
  it took.

### Chat

- Channels and direct messages, with their history kept while you were away;
  replies, reactions and resending, and one language of marks for what
  happened to each message, on both meshes.
- MeshCore's rooms, logged into with or without a password.

### Settings

- A Meshtastic radio's user, LoRa, channels, device and modules; a MeshCore
  radio's own settings, channels and regions.
- Nodes, channels and a radio's LoRa added from their links, pasted, scanned
  as a QR code with the camera, or read from a photo.
- meshctl's own: the radios, what it keeps and for how long, and how it looks
  (light or dark, and its accent), kept in its database.

### Running it

- `meshctld`, one binary with the interface inside, for Linux (x86_64,
  aarch64) and macOS; a container image, for amd64 and arm64.
- A desktop app for Windows (portable) and Linux (Flatpak), that runs the
  daemon within itself, keeps listening with its window closed, and opens
  `meshcore://` links.
- The panel installs as an app on a phone, and takes links shared to it.
- An access token, which a proxy in front can add to every request; an API
  that every client uses, documented at `/api/docs`.
- In English and Spanish.
