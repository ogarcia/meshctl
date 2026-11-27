# Bundled browser libraries

The interface serves these from the binary itself: nothing is fetched from a CDN.
They are unmodified upstream files, but for one line: the comment at the end
of MapLibre's two `.mjs` files that names their source maps, which are not
kept, taken away, so that a browser's tools do not ask for them in vain.

| Library | Version | Source | SHA-256 of the archive | Licence |
|---|---|---|---|---|
| Leaflet | 1.9.4 | https://registry.npmjs.org/leaflet/-/leaflet-1.9.4.tgz | `84c65a256e50657896f54c33bd857b6849ebe94c817803be818bf32a3dde0b77` | BSD-2-Clause (`leaflet-1.9.4/LICENSE`) |
| MapLibre GL JS | 6.18.0 | https://registry.npmjs.org/maplibre-gl/-/maplibre-gl-6.18.0.tgz | `9486aea5ae37e47b32ce356ad1ff1d1ab3274e908a562cc61e668c3c8cc60243` | BSD-3-Clause (`maplibre-gl-6.18.0/LICENSE.txt`) |
| Tabler Icons | 3.51.0 | https://registry.npmjs.org/@tabler/icons/-/icons-3.51.0.tgz | `b30108713d3fa60aee862c2a133225da8e0f309be0708fe2925763ff810c766c` | MIT (`tabler-icons-3.51.0/LICENSE`) |
| Emojibase data | 18.0.0 | https://registry.npmjs.org/emojibase-data/-/emojibase-data-18.0.0.tgz | `ed207c64f31baab5db419686135be3696bbd196a88228a3564383d7918db8f34` | MIT (`emojibase-data-18.0.0/LICENSE`) |

From Leaflet only `dist/leaflet.js`, `dist/leaflet.css` and the images the
stylesheet names are kept. From MapLibre only `dist/maplibre-gl.mjs`,
`dist/maplibre-gl-worker.mjs` (which the first loads by its own address) and
`dist/maplibre-gl.css`: `map-maplibre.js` loads the first the first time a
vector map is wanted. From Tabler, only the SVG files the interface uses,
as `icons/outline/<name>.svg`: add one there when it is needed. The interface
builds some of them into itself, as the icons of the radios' buttons. From
Emojibase, only `en/compact.json` and `es/compact.json`, one per language of
the interface: the emoji picker reads the one it speaks.

The two icons that stand for Meshtastic and MeshCore radios are not these
projects' logos, which are trademarks: they are drawn for meshctl, in
`web/icons`.

To update one, download its versioned npm archive and check its checksum. Copy
the same files into a new versioned directory, point `index.html` at it (and,
for Emojibase, `DATA` in `src/emoji.rs`; for Tabler, the paths in `src`), and
remove the old one.

The maps still come from the network: OpenStreetMap's tiles, by default, or
OpenFreeMap's vector styles and tiles, made from OpenStreetMap's data, as the
preferences say.
