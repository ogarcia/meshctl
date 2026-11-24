# meshctl's website

The website of meshctl, served at <https://meshctl.ogarcia.me> by GitHub Pages,
which `.github/workflows/pages.yml` publishes from this directory as it stands
on every change to it in `master`.

Plain HTML and one stylesheet, `meshctl.css`, with nothing to build, as
Meshview's is. It is made of meshctl's own pieces: its bars, buttons and cards,
and the colours of both its themes, light or dark as the browser is, in its
amber; with the Inter typeface (`fonts/`, SIL Open Font License) and Tabler's
icons (`icons/`, MIT). The flag in the foot is an SVG in `flags/`, as emoji
flags do not show on Windows. The English page is `index.html` and the Spanish
one `es/index.html`; they say the same, and a change to one goes to the other.

The screenshots in `shots/` are WebP, from the panel in its dark theme: the
desktop ones 1600×900, the phone ones twice a 393-pixel-wide screen. The mesh
is the one around A Coruña that a radio there hears, its map light; the chat,
the link window and the repeater's page come from the fake MeshCore radio in
`core/examples/`, its channels Galicia's and its messages written for them, in
English for the English page and in Spanish for the Spanish one. `og.png` is
the picture a link to the site shows where it is pasted, 1200×630.

To look at it before publishing:

```sh
python3 -m http.server 8000
```
