# meshctl

A lean control panel for your mesh radios: control them, see the mesh around
them, chat, and trace routes. That is the whole list.

meshctl stays connected to your radios around the clock, keeps what they hear
in SQLite, and serves a web interface and an API from a single binary. One
radio or a dozen, Meshtastic and MeshCore, all from the same panel.

> [!WARNING]
> **meshctl is just starting, and it is not stable at all.** It changes, and
> breaks, from one commit to the next: the interface, the API and the
> database alike. There are no releases yet, and no promise that a database
> made today opens with tomorrow's build. Try it if you are curious; do not
> rely on it for anything that matters.

## Goals

- **Your radios:** as many as you have, added and changed from the panel.
- **Each node:** its identity, role, channels and LoRa settings.
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

Meshtastic comes first. MeshCore radios connect, and show their nodes and
chats, but they can do less than Meshtastic ones for now.

## Running

```sh
meshctl
```

That is all it needs. It keeps its database in `./data` (`--data` or
`MESHCTL_DATA` to put it elsewhere) and, the first time, the panel asks for a
radio. A radio's address is one of:

```text
192.168.1.10              on the network, port 4403 (5000 for MeshCore)
radio.lan:4404            on the network, another port
/dev/ttyACM0              plugged in, at 115200
/dev/ttyUSB0@921600       plugged in, at another speed
```

A radio serves one client at a time. If something else (the phone app, say)
also needs it, put [tasticsplitter](https://github.com/ogarcia/tasticsplitter)
in front and give meshctl its address instead.

### In a container

Every push to master builds an image, for amd64 and arm64:

```sh
podman run -d --name meshctl -p 7434:7434 \
    -v meshctl:/var/lib/meshctl \
    -e MESHCTL_TOKEN=a-long-secret \
    ghcr.io/ogarcia/meshctl:master
```

In the container, meshctl listens on every interface, port 7434, and keeps its
database in `/var/lib/meshctl`, the volume above. Set a token, or put a proxy
in front (see [Access](#access)).

A radio on USB is a device passed in, which the container's user must be
allowed to open. With rootless podman, keep your own groups:

```sh
podman run … --device /dev/ttyACM0 --group-add keep-groups …
```

With docker, add the group that owns the device:

```sh
docker run … --device /dev/ttyACM0 --group-add "$(stat -c %g /dev/ttyACM0)" …
```

`master` follows master, and the warning above with it: a newer image may not
open the database an older one made. If it does not start after an update,
remove the volume and add your radios again.

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
cargo run -p meshctl-core --example probe -- <host> [port] [seconds]
                                       # what a radio says, line by line;
                                       # read only, never transmits
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
