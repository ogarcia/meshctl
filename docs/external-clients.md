# External clients

meshctl serves a local HTTP API and a WebSocket event stream. External clients
should use these interfaces rather than opening a second connection to a radio
or reading meshctl's SQLite database.

The current client-facing surface is:

| Purpose | Method | Path |
| --- | --- | --- |
| Version | GET | `/api/v1/version` |
| Radios | GET | `/api/v1/radios` |
| Radio status | GET | `/api/v1/radios/{radio}/status` |
| Channels | GET | `/api/v1/radios/{radio}/channels` |
| Nodes | GET | `/api/v1/radios/{radio}/nodes` |
| Node details | GET | `/api/v1/radios/{radio}/nodes/{node}` |
| Packets | GET | `/api/v1/radios/{radio}/packets` |
| Packet routes | GET | `/api/v1/radios/{radio}/packets/{id}/routes` |
| Live events | WebSocket | `/api/v1/events` |
| Send text | POST | `/api/v1/radios/{radio}/messages` |
| Request node data | POST | `/api/v1/radios/{radio}/requests` |

The complete generated reference is available at `/api/openapi.json` and
`/api/docs`.

## Authentication

When `MESHCTL_TOKEN` is set, HTTP clients send it in `X-Meshctl-Token`. The
WebSocket client sends the token in its initial JSON hello message. Clients
should treat `401` as an authentication failure and preserve the structured
error body.

## Safety

GET endpoints are observational. POST endpoints can cause radio activity and
must be treated as explicit operator actions. Clients should show the radio,
destination, payload or request kind, and expected effect before invoking
them. Clients should never connect directly to a radio while meshctl is using
it.

This document describes the supported surface for external clients; it does
not make the clients part of meshctl or require a particular client framework.
