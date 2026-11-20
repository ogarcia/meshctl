# meshctl

A lean control panel for your Meshtastic node: control it, see the mesh around
it, chat, and trace routes. That is the whole list.

meshctl stays connected to your node around the clock, keeps what it hears in
SQLite, and serves a web interface and an API from a single binary.

## Goals

- **Your node:** its identity, role, channels and LoRa settings.
- **The mesh:** every node heard, when, how well and how far, on a list and a
  map.
- **Chat:** channels and direct messages, with history kept while you were away.
- **Traceroutes:** on demand, with their history.
- **One binary**, small enough for a Raspberry Pi or a router, with the web
  interface built in.
- **An API** that every client uses, the web interface included, documented at
  `/api/docs`.

## Non-goals

Written down so that they stay that way. A feature request for any of these is
a reason to point at another tool, not to grow this one.

- Bots, automations, auto-responders or scheduled messages.
- MQTT, Matrix or any other bridge. See
  [loratrix](https://github.com/ogarcia/loratrix) for that.
- Telemetry dashboards, charts or statistics beyond what a node says about
  itself.
- Network-wide monitoring and analysis. See
  [Meshview](https://github.com/meshview/meshview) for that.
- Users, roles and permissions. See [Access](#access).
- Sharing one radio between several clients. Put
  [tasticsplitter](https://github.com/ogarcia/tasticsplitter) in front of it,
  and point meshctl at it as if it were the radio.

MeshCore support is planned, after Meshtastic works well.

## Access

meshctl listens on `127.0.0.1:7434` by default, so only the machine it runs on
can reach it. To reach it from elsewhere, set `MESHCTL_LISTEN` (for example
`0.0.0.0:7434`) and protect it in one of two ways:

- **A reverse proxy with its own authentication** in front (nginx or Caddy with
  basic auth, Authelia, Authentik…). Use HTTPS if it is reachable from outside
  your network.
- **An access token**: set `MESHCTL_TOKEN`, and clients send it in the
  `X-Meshctl-Token` header. It does not use `Authorization`, so it can be
  combined with a proxy's basic auth.

meshctl warns at startup when it listens beyond this machine without a token.

## Building

```sh
# The interface first, so that the server embeds it.
(cd web && trunk build --release)
cargo build --release
```

The binary is `target/release/meshctl`. Built without the interface, it still
serves the API and tells you at `/` what is missing.

The web interface needs [trunk](https://trunkrs.dev) and the
`wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`).

## Development

```sh
git config core.hooksPath .githooks   # format and clippy before each commit
cargo run                              # the daemon, on 127.0.0.1:7434
(cd web && trunk serve)                # the interface on 127.0.0.1:7435,
                                       # rebuilt on change, API proxied
```

| Crate    | What it is                                                     |
| -------- | -------------------------------------------------------------- |
| `proto`  | Meshtastic protobufs (vendored) and the stream framing         |
| `core`   | The link to a radio over TCP or serial, and the domain model   |
| `api`    | The types the daemon and its clients share; compiles to WASM   |
| `server` | The daemon, the `meshctl` binary                               |
| `web`    | The Leptos interface (client-side rendered), with translations |

Translations are in `web/locales/`, one JSON file per language, English being
the reference. Using a key English does not declare is a compile error; one
another language lacks is a build warning, and shows in English meanwhile.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
