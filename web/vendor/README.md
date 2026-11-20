# Bundled browser libraries

The interface serves these from the binary itself: nothing is fetched from a CDN.
They are unmodified upstream files.

| Library | Version | Source | SHA-256 of the archive | Licence |
|---|---|---|---|---|
| Leaflet | 1.9.4 | https://registry.npmjs.org/leaflet/-/leaflet-1.9.4.tgz | `84c65a256e50657896f54c33bd857b6849ebe94c817803be818bf32a3dde0b77` | BSD-2-Clause (`leaflet-1.9.4/LICENSE`) |

From Leaflet only `dist/leaflet.js`, `dist/leaflet.css` and the images the
stylesheet names are kept.

To update one, download its versioned npm archive and check its checksum. Copy
the same files into a new versioned directory, point `index.html` at it, and
remove the old one.

The map tiles still come from the network: OpenStreetMap's, by default.
