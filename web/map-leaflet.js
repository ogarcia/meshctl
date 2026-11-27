// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

// The maps' engine, with Leaflet and OpenStreetMap's tiles: what map.js asks
// of a map, and nothing of what it shows. It is the only file that knows
// Leaflet, so that another engine can stand in its place by answering the
// same: create(element, options) gives a map, and the map these.
//
//   remove(), resize()
//   setView(lat, lon, zoom, animate), zoom(), centre() {lat, lon}
//   bounds() {south, west, north, east}, size() {x, y}
//   fit(points, {padding, paddingTopLeft, paddingBottomRight, maxZoom,
//       animate, fly, duration}), fitZoom(points, padding)
//   toPoint(lat, lon) {x, y} and toLatLon(x, y) {lat, lon}, on the screen
//   on(name, handler): click {x, y, lat, lon, event}, moveend, zoomstart,
//       zoomend, keydown (event), popupclose (the popup)
//   layer(): a group to draw on, with marker, line, ring, circle, clear
//       and remove; the map is one too, for what goes on it alone
//   popup(lat, lon, content, options), closePopup(popup?)
//   control(element, position)
//   stop(event): a click handled, which the map is not to take as its own
//
// Points are [lat, lon]; pixels {x, y} from the map's top left corner.
window.meshctlLeaflet = (() => {
  // OpenStreetMap's tiles, as their usage policy asks to be credited.
  const TILES = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
  const ATTRIBUTION =
    '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>';

  // A map on an element: one to move about, or a still one (still), as the
  // small maps of the nodes' pages, which nothing moves but the page.
  function create(element, options = {}) {
    const still = !!options.still;
    const map = L.map(element, {
      zoomSnap: options.zoomSnap ?? 1,
      zoomControl: !still && options.zoomControl !== false,
      worldCopyJump: !still,
      ...(still
        ? {
            dragging: false,
            scrollWheelZoom: false,
            doubleClickZoom: false,
            boxZoom: false,
            keyboard: false,
            touchZoom: false,
            tap: false,
          }
        : {}),
    });
    map.attributionControl.setPrefix(false);
    L.tileLayer(TILES, { maxZoom: 19, attribution: ATTRIBUTION }).addTo(map);
    return wrap(map);
  }

  // Padding as Leaflet takes it: [x, y].
  const pair = (padding) =>
    Array.isArray(padding) ? padding : [padding || 0, padding || 0];

  // Leaflet's bounds of a list of points, or of [[south, west], [north,
  // east]].
  const boundsOf = (points) => L.latLngBounds(points);

  function wrap(map) {
    // The popups opened, by Leaflet's, to say which one closed.
    const popups = new WeakMap();
    const self = {
      remove: () => map.remove(),
      resize: () => map.invalidateSize(),
      setView: (lat, lon, zoom, animate = true) =>
        map.setView([lat, lon], zoom, { animate }),
      zoom: () => map.getZoom(),
      centre: () => {
        const c = map.getCenter();
        return { lat: c.lat, lon: c.lng };
      },
      bounds: () => {
        const b = map.getBounds();
        return { south: b.getSouth(), west: b.getWest(), north: b.getNorth(), east: b.getEast() };
      },
      size: () => map.getSize(),
      fit: (points, o = {}) => {
        const options = { maxZoom: o.maxZoom, animate: o.animate !== false };
        if (o.padding !== undefined) options.padding = pair(o.padding);
        if (o.paddingTopLeft) options.paddingTopLeft = o.paddingTopLeft;
        if (o.paddingBottomRight) options.paddingBottomRight = o.paddingBottomRight;
        if (o.fly) {
          map.flyToBounds(boundsOf(points), { ...options, duration: o.duration });
        } else {
          map.fitBounds(boundsOf(points), options);
        }
      },
      // The zoom fit() goes to, worked out as it does.
      fitZoom: (points, padding) =>
        map.getBoundsZoom(boundsOf(points), false, L.point(pair(padding))),
      toPoint: (lat, lon) => map.latLngToContainerPoint([lat, lon]),
      toLatLon: (x, y) => {
        const at = map.containerPointToLatLng([x, y]);
        return { lat: at.lat, lon: at.lng };
      },
      on: (name, handler) => {
        if (name === "click") {
          map.on("click", (e) =>
            handler({
              x: e.containerPoint.x,
              y: e.containerPoint.y,
              lat: e.latlng.lat,
              lon: e.latlng.lng,
              event: e.originalEvent,
            }),
          );
        } else if (name === "keydown") {
          map.on("keydown", (e) => handler(e.originalEvent));
        } else if (name === "popupclose") {
          map.on("popupclose", (e) => handler(popups.get(e.popup) || null));
        } else {
          map.on(name, () => handler());
        }
      },
      popup: (lat, lon, content, o = {}) => {
        // Only what is given: one left undefined would stand for Leaflet's
        // own default, and break it.
        const options = { closeButton: o.closeButton !== false };
        for (const name of ["className", "maxWidth", "maxHeight", "offset", "autoPanPadding"]) {
          if (o[name] !== undefined) options[name] = o[name];
        }
        const popup = L.popup(options)
          .setLatLng([lat, lon])
          .setContent(content);
        const handle = {
          content: () => popup.getContent(),
          at: () => {
            const at = popup.getLatLng();
            return { lat: at.lat, lon: at.lng };
          },
          move: (lat, lon) => popup.setLatLng([lat, lon]),
          // Placed again over what it is on, its content changed in place.
          reposition: () => popup._updatePosition && popup._updatePosition(),
          // Brought back within the map, if it was left out of it.
          fit: () => popup._adjustPan && popup._adjustPan(),
          close: () => map.closePopup(popup),
        };
        popups.set(popup, handle);
        popup.openOn(map);
        return handle;
      },
      closePopup: (handle) => (handle ? handle.close() : map.closePopup()),
      control: (element, position = "topleft") => {
        const Control = L.Control.extend({
          options: { position },
          onAdd() {
            L.DomEvent.disableClickPropagation(element);
            return element;
          },
        });
        new Control().addTo(map);
      },
      stop: (event) => L.DomEvent.stop(event),
      layer: () => group(map, L.layerGroup().addTo(map)),
    };
    return self;
  }

  // What is drawn on a group: markers of the page's own elements, lines,
  // rings of a size on the screen and circles of one on the ground.
  //
  //   marker(lat, lon, {html, size: [w, h], anchor: [x, y], tooltip,
  //          tooltipAnchor, z, interactive, draggable, moved(lat, lon)})
  //   line(points, style), ring(lat, lon, pixels, style),
  //   circle(lat, lon, metres, style), clear(), remove()
  //
  // A style is {color, weight, opacity, dash, cap, fill, fillOpacity}: a
  // dash as "8 6", fill a colour (none, no fill).
  function group(map, layer) {
    const lineStyle = (s) => ({
      color: s.color,
      weight: s.weight,
      opacity: s.opacity ?? 1,
      dashArray: s.dash || null,
      lineCap: s.cap || "round",
      fill: !!s.fill,
      fillColor: s.fill,
      fillOpacity: s.fillOpacity ?? 0.2,
      interactive: false,
    });
    return {
      marker: (lat, lon, o) => {
        const marker = L.marker([lat, lon], {
          icon: L.divIcon({
            className: "",
            html: o.html,
            iconSize: o.size,
            iconAnchor: o.anchor,
            ...(o.tooltipAnchor ? { tooltipAnchor: o.tooltipAnchor } : {}),
          }),
          keyboard: false,
          interactive: o.interactive !== false || !!o.draggable,
          draggable: !!o.draggable,
          zIndexOffset: o.z || 0,
          // Clicks reach the map, which looks for whatever is near them.
          bubblingMouseEvents: true,
        });
        if (o.tooltip) {
          marker.bindTooltip(o.tooltip, { direction: "top", className: "mc-tip" });
        }
        if (o.moved) {
          marker.on("dragend", () => {
            const at = marker.getLatLng().wrap();
            o.moved(at.lat, at.lng);
          });
        }
        marker.addTo(layer);
        return {
          move: (lat, lon) => marker.setLatLng([lat, lon]),
          remove: () => marker.remove(),
        };
      },
      line: (points, style) => L.polyline(points, lineStyle(style)).addTo(layer),
      ring: (lat, lon, pixels, style) =>
        L.circleMarker([lat, lon], { ...lineStyle(style), radius: pixels }).addTo(layer),
      circle: (lat, lon, metres, style) =>
        L.circle([lat, lon], { ...lineStyle(style), radius: metres }).addTo(layer),
      clear: () => layer.clearLayers(),
      remove: () => layer.remove(),
    };
  }

  return { create };
})();
