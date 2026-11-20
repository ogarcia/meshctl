The API behind meshctl's panel, for any other client too.

Everything lives under `/api/v1`. Reading is `GET`; anything that changes the
radio or sends over the air uses another method, so that a proxy in front can
open the reading part to everyone and keep the rest to itself.

**Access.** When the daemon is given a token, every request carries it in the
`X-Meshctl-Token` header, and the event stream in its first message.

**Codes, not words.** Errors, link states, roles, hardware models, regions and
presets travel as codes, the latter as the Meshtastic protobufs name them
(`HELTEC_V3`, `CLIENT_BASE`, `EU_868`). Each client puts them in its own words.

**Following changes.** Open the event stream at `/api/v1/events`, send
`{}` (or `{"token": "…"}`), wait for `{"type": "ready"}`, and only then read
the state with `GET`: from there on every change arrives as an event. On
`synced` or `lagged`, read the state again.
