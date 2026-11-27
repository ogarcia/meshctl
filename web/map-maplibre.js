// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

// The maps' other engine, with MapLibre and OpenFreeMap's vector styles,
// made from OpenStreetMap's data: sharp at any zoom. It answers what
// map-leaflet.js does, as that file says, and one more: ready(), a promise
// of MapLibre loaded, which it is the first time a map is wanted.
//
// Zooms are given and told as Leaflet's, of tiles 256 pixels wide: MapLibre
// counts one less for the same scale, its tiles being of 512.
//
// Never with OpenStreetMap's own tiles, which MapLibre draws blurred: those
// are the other engine's.
window.meshctlMapLibre = (() => {
  const LIBRARY = "/maplibre-gl-6.18.0/dist/maplibre-gl.mjs";
  const OPENFREEMAP = "https://tiles.openfreemap.org/styles/";
  let ml = null;
  let loading = null;

  // MapLibre, loaded once, as it is first wanted.
  function ready() {
    if (!loading) {
      loading = import(LIBRARY).then((library) => {
        ml = library;
      });
    }
    return loading;
  }

  // Whether the maps are dark: as asked for, or following the interface's
  // theme, given or the system's.
  function dark() {
    const root = document.documentElement;
    const map = root.getAttribute("data-map-theme");
    if (map) return map === "dark";
    const theme = root.getAttribute("data-theme");
    if (theme) return theme === "dark";
    return matchMedia("(prefers-color-scheme: dark)").matches;
  }

  // The style, as the preferences say: detailed, Liberty, or Fiord on a
  // dark map; vivid, Bright, made dark on a dark map, as it has no dark of
  // its own; plain, Positron, or Dark. As a key, its address and whether it
  // is made dark.
  function styleUrl() {
    const colors = document.documentElement.getAttribute("data-map-colors");
    const night = dark();
    if (colors === "vivid") return OPENFREEMAP + (night ? "bright#dark" : "bright");
    if (colors === "gray") return OPENFREEMAP + (night ? "dark" : "positron");
    return OPENFREEMAP + (night ? "fiord" : "liberty");
  }

  // A style to load: its address, and whether its colours are made dark.
  function loaded(key) {
    const [url, how] = key.split("#");
    return { url, darkened: how === "dark" };
  }

  // A colour made dark as OpenStreetMap's tiles are, by the very filter
  // they go through: invert(1) hue-rotate(180deg) brightness(0.85)
  // saturate(0.75). Any colour CSS reads; none, anything else.
  const painter = document.createElement("canvas").getContext("2d");
  function darkened(css) {
    if (typeof css !== "string" || !/^(#|rgb|hsl)/.test(css)) return null;
    painter.fillStyle = "#010203";
    painter.fillStyle = css;
    const read = painter.fillStyle;
    if (read === "#010203" && css.toLowerCase() !== "#010203") return null;
    let r;
    let g;
    let b;
    let a = 1;
    if (read.startsWith("#")) {
      [r, g, b] = [1, 3, 5].map((i) => parseInt(read.slice(i, i + 2), 16));
    } else {
      const parts = read.match(/[\d.]+/g).map(Number);
      [r, g, b] = parts;
      if (parts.length > 3) a = parts[3];
    }
    // Inverted, and its hue turned half round.
    [r, g, b] = [255 - r, 255 - g, 255 - b];
    [r, g, b] = [
      -0.574 * r + 1.43 * g + 0.144 * b,
      0.426 * r + 0.43 * g + 0.144 * b,
      0.426 * r + 1.43 * g - 0.856 * b,
    ].map((v) => Math.min(255, Math.max(0, v)));
    // Dimmed, and less saturated.
    [r, g, b] = [r, g, b].map((v) => v * 0.85);
    const s = 0.75;
    [r, g, b] = [
      (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
      (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
      (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ].map((v) => Math.round(Math.min(255, Math.max(0, v))));
    return `rgba(${r}, ${g}, ${b}, ${a})`;
  }

  // Every colour of a paint's value made dark, within expressions too.
  function darkenAll(value) {
    if (Array.isArray(value)) return value.map(darkenAll);
    return darkened(value) ?? value;
  }

  // A style's filters, made quiet: OpenFreeMap's shields ask whether a
  // road's reference is short, of roads that may have none, and MapLibre
  // warns of each; one with none is taken as long, which it leaves out as
  // well, warning of nothing.
  function quiet(expression) {
    if (!Array.isArray(expression)) return expression;
    if (expression.length === 2 && expression[0] === "get" && expression[1] === "ref_length") {
      return ["coalesce", expression, 99];
    }
    return expression.map(quiet);
  }

  function tidy(_before, style) {
    for (const layer of style.layers || []) {
      if (layer.filter) layer.filter = quiet(layer.filter);
    }
    return style;
  }

  // The same, and its colours made dark.
  function tidyDark(before, style) {
    tidy(before, style);
    for (const layer of style.layers || []) {
      for (const [name, value] of Object.entries(layer.paint || {})) {
        if (name.endsWith("color")) layer.paint[name] = darkenAll(value);
      }
    }
    return style;
  }

  // Gives a map a style, by its key, made quiet, and dark if it is to be.
  function styled(map, key) {
    const { url, darkened: dim } = loaded(key);
    map.setStyle(url, { transformStyle: dim ? tidyDark : tidy });
  }

  // An image a style asks for that its sprite has not: given as nothing,
  // which is what is drawn of it anyway, rather than warned of.
  const NOTHING = { width: 1, height: 1, data: new Uint8Array(4) };

  const lngLat = (lat, lon) => [lon, lat];

  // The area that holds places given as [lat, lon], or [[south, west],
  // [north, east]].
  function boundsOf(points) {
    const bounds = new ml.LngLatBounds();
    for (const [lat, lon] of points) bounds.extend([lon, lat]);
    return bounds;
  }

  // Padding as MapLibre takes it, from Leaflet's: the same on every side, or
  // [x, y] on each, or top left and bottom right apart.
  function paddingOf(o) {
    const all = (p) => (Array.isArray(p) ? p : [p || 0, p || 0]);
    if (o.paddingTopLeft || o.paddingBottomRight) {
      const [left, top] = all(o.paddingTopLeft);
      const [right, bottom] = all(o.paddingBottomRight);
      return { top, bottom, left, right };
    }
    const [x, y] = all(o.padding);
    return { top: y, bottom: y, left: x, right: x };
  }

  // A circle on the ground as a polygon: its radius in metres.
  function circlePolygon(lat, lon, metres) {
    const points = [];
    const dLat = metres / 111320;
    const dLon = metres / (111320 * Math.max(Math.cos((lat * Math.PI) / 180), 0.01));
    for (let i = 0; i <= 64; i++) {
      const a = (i / 64) * 2 * Math.PI;
      points.push([lon + dLon * Math.cos(a), lat + dLat * Math.sin(a)]);
    }
    return points;
  }

  // A dash as Leaflet takes it ("8 6", in pixels) as MapLibre does, in
  // widths of the line.
  const dashOf = (dash, weight) =>
    dash ? dash.split(/[ ,]+/).map((n) => Number(n) / (weight || 1)) : null;

  let next = 0;

  function create(element, options = {}) {
    const still = !!options.still;
    const map = new ml.Map({
      container: element,
      center: [0, 20],
      zoom: 1,
      attributionControl: false,
      dragRotate: false,
      pitchWithRotate: false,
      touchPitch: false,
      maxPitch: 0,
      interactive: !still,
    });
    // Given apart, to be made quiet.
    styled(map, styleUrl());
    if (map.touchZoomRotate) map.touchZoomRotate.disableRotation();
    if (map.keyboard) map.keyboard.disableRotation();
    map.setMissingStyleImageResolver((id) => {
      if (!map.hasImage(id)) map.addImage(id, NOTHING);
    });
    map.addControl(new ml.AttributionControl({ compact: false }), "bottom-right");
    if (!still && options.zoomControl !== false) map.addControl(zoomControl(map), "top-left");
    return wrap(map, element);
  }

  // The zoom's buttons, as Leaflet's are, to look the same whichever draws.
  function zoomControl(map) {
    return {
      onAdd() {
        const bar = document.createElement("div");
        bar.className = "maplibregl-ctrl leaflet-bar leaflet-control-zoom";
        for (const [className, text, step] of [
          ["leaflet-control-zoom-in", "+", 1],
          ["leaflet-control-zoom-out", "−", -1],
        ]) {
          const button = document.createElement("a");
          button.className = className;
          button.href = "#";
          button.setAttribute("role", "button");
          button.textContent = text;
          button.addEventListener("click", (event) => {
            event.preventDefault();
            event.stopPropagation();
            map.easeTo({ zoom: map.getZoom() + step, duration: 250 });
          });
          bar.append(button);
        }
        return bar;
      },
      onRemove() {},
    };
  }

  function wrap(map, element) {
    const handlers = { click: [], contextmenu: [], moveend: [], zoomstart: [], zoomend: [], keydown: [], popupclose: [] };
    const fire = (name, value) => handlers[name].forEach((h) => h(value));
    // What a click on a marker, or a touch, is to the map: its point.
    const clicked = (x, y, event) => {
      const at = map.unproject([x, y]);
      return { x, y, lat: at.lat, lon: at.lng, event };
    };
    // A click a marker took for the map, not to be taken twice.
    let lastMarkerClick = 0;
    map.on("click", (e) => {
      if (performance.now() - lastMarkerClick < 400) return;
      fire("click", clicked(e.point.x, e.point.y, e.originalEvent));
    });
    map.on("contextmenu", (e) => fire("contextmenu", clicked(e.point.x, e.point.y, e.originalEvent)));
    map.on("moveend", () => fire("moveend"));
    map.on("zoomstart", () => fire("zoomstart"));
    map.on("zoomend", () => fire("zoomend"));
    element.addEventListener("keydown", (event) => fire("keydown", event));

    // What the groups drew, to draw again as the style changes, which takes
    // every layer of its own away. Whether the style is there to draw on:
    // not while it is being loaded, but while its tiles are.
    const groups = new Set();
    let styleReady = false;
    let url = styleUrl();
    const styled = () => styleReady;
    map.on("style.load", () => {
      styleReady = true;
      groups.forEach((g) => g.redraw());
    });

    // One popup at a time, as Leaflet has it.
    let popupNow = null;

    const self = {
      // The style again, if the preferences or the system's theme changed it.
      restyle: () => {
        const now = styleUrl();
        if (now === url) return;
        url = now;
        styleReady = false;
        styled(map, now);
      },
      remove: () => map.remove(),
      resize: () => map.resize(),
      setView: (lat, lon, zoom, animate = true) =>
        map[animate ? "easeTo" : "jumpTo"]({ center: lngLat(lat, lon), zoom: zoom - 1 }),
      zoom: () => map.getZoom() + 1,
      centre: () => {
        const c = map.getCenter();
        return { lat: c.lat, lon: c.lng };
      },
      bounds: () => {
        const b = map.getBounds();
        return { south: b.getSouth(), west: b.getWest(), north: b.getNorth(), east: b.getEast() };
      },
      size: () => ({ x: element.clientWidth, y: element.clientHeight }),
      fit: (points, o = {}) => {
        const options = {
          padding: paddingOf(o),
          animate: o.animate !== false,
        };
        if (o.maxZoom !== undefined) options.maxZoom = o.maxZoom - 1;
        if (o.fly) options.duration = (o.duration || 0.5) * 1000;
        try {
          map.fitBounds(boundsOf(points), options);
        } catch (_) {
          // Padding wider than the map, as while it is laid out.
        }
      },
      fitZoom: (points, padding) => {
        const [x, y] = Array.isArray(padding) ? padding : [padding || 0, padding || 0];
        const camera = map.cameraForBounds(boundsOf(points), {
          padding: { top: y / 2, bottom: y / 2, left: x / 2, right: x / 2 },
        });
        return camera ? camera.zoom + 1 : map.getZoom() + 1;
      },
      toPoint: (lat, lon) => map.project(lngLat(lat, lon)),
      toLatLon: (x, y) => {
        const at = map.unproject([x, y]);
        return { lat: at.lat, lon: at.lng };
      },
      on: (name, handler) => handlers[name] && handlers[name].push(handler),
      popup: (lat, lon, content, o = {}) => {
        if (popupNow) popupNow.close();
        const popup = new ml.Popup({
          className: o.className,
          closeButton: o.closeButton !== false,
          closeOnClick: true,
          focusAfterOpen: false,
          maxWidth: o.maxWidth ? `${o.maxWidth}px` : "none",
          offset: o.offset ? [o.offset[0], o.offset[1]] : 0,
        })
          .setLngLat(lngLat(lat, lon))
          .setDOMContent(content)
          .addTo(map);
        const body = popup.getElement().querySelector(".maplibregl-popup-content");
        if (o.maxHeight && body) {
          body.style.maxHeight = `${o.maxHeight}px`;
          body.style.overflowY = "auto";
        }
        const handle = {
          content: () => content,
          at: () => {
            const at = popup.getLngLat();
            return { lat: at.lat, lon: at.lng };
          },
          move: (lat, lon) => popup.setLngLat(lngLat(lat, lon)),
          reposition: () => {},
          // Brought within the map, if it is out of it.
          fit: () => fitPopup(map, element, popup),
          close: () => popup.remove(),
        };
        popup.on("close", () => {
          if (popupNow === handle) popupNow = null;
          fire("popupclose", handle);
        });
        popupNow = handle;
        fitPopup(map, element, popup);
        return handle;
      },
      closePopup: (handle) => {
        if (handle) handle.close();
        else if (popupNow) popupNow.close();
      },
      control: (bar, position = "topleft") => {
        bar.classList.add("maplibregl-ctrl");
        // Clicks on it are its own, not the map's.
        bar.addEventListener("click", (event) => event.stopPropagation());
        map.addControl(
          { onAdd: () => bar, onRemove: () => bar.remove() },
          position === "topleft" ? "top-left" : position,
        );
      },
      stop: (event) => {
        event.stopPropagation();
        event.preventDefault();
      },
      layer: () => {
        const g = group(map, styled, () => {
          lastMarkerClick = performance.now();
        }, (x, y, event) => fire("click", clicked(x, y, event)));
        groups.add(g);
        return g;
      },
    };
    return self;
  }

  // Brings a popup within the map, panning it if it is out of it.
  function fitPopup(map, element, popup) {
    requestAnimationFrame(() => {
      const shown = popup.getElement();
      if (!shown || !shown.isConnected) return;
      const area = element.getBoundingClientRect();
      const box = shown.getBoundingClientRect();
      const margin = 24;
      let dx = 0;
      let dy = 0;
      if (box.left < area.left + margin) dx = box.left - area.left - margin;
      else if (box.right > area.right - margin) dx = box.right - area.right + margin;
      if (box.top < area.top + margin) dy = box.top - area.top - margin;
      else if (box.bottom > area.bottom - margin) dy = box.bottom - area.bottom + margin;
      if (dx || dy) map.panBy([dx, dy], { duration: 250 });
    });
  }

  // What is drawn on a group: markers of the page's own elements, and lines,
  // rings and areas, each a layer of its own, drawn again as the style
  // changes. A click on a marker that takes none goes to the map, as on
  // Leaflet's: told by `clickMap`, its point within the map.
  function group(map, styled, markerClicked, clickMap) {
    const id = `mc-${next++}`;
    let shapes = [];
    let markers = [];
    let count = 0;

    const addShape = (shape) => {
      if (!styled()) return;
      map.addSource(shape.id, { type: "geojson", data: shape.data });
      for (const layer of shape.layers) map.addLayer({ ...layer, source: shape.id });
    };
    const removeShape = (shape) => {
      for (const layer of shape.layers) if (map.getLayer(layer.id)) map.removeLayer(layer.id);
      if (map.getSource(shape.id)) map.removeSource(shape.id);
    };
    const keep = (shape) => {
      shapes.push(shape);
      addShape(shape);
    };

    // A line's or an area's edge, as its style says.
    const lineLayer = (layerId, style, filter) => {
      const paint = {
        "line-color": style.color,
        "line-width": style.weight ?? 3,
        "line-opacity": style.opacity ?? 1,
      };
      const dash = dashOf(style.dash, style.weight);
      if (dash) paint["line-dasharray"] = dash;
      const layer = {
        id: layerId,
        type: "line",
        layout: { "line-cap": style.cap || "round", "line-join": "round" },
        paint,
      };
      if (filter) layer.filter = filter;
      return layer;
    };
    const area = (ring, style) => {
      const shapeId = `${id}-${count++}`;
      const layers = [];
      if (style.fill) {
        layers.push({
          id: `${shapeId}-fill`,
          type: "fill",
          paint: { "fill-color": style.fill, "fill-opacity": style.fillOpacity ?? 0.2 },
        });
      }
      layers.push(lineLayer(`${shapeId}-line`, style));
      keep({
        id: shapeId,
        data: { type: "Feature", geometry: { type: "Polygon", coordinates: [ring] } },
        layers,
      });
    };

    return {
      redraw: () => shapes.forEach(addShape),
      marker: (lat, lon, o) => {
        const holder = document.createElement("div");
        holder.innerHTML = o.html;
        holder.style.width = `${o.size[0]}px`;
        holder.style.height = `${o.size[1]}px`;
        if (o.z) holder.style.zIndex = String(1000 + o.z);
        if (o.interactive === false && !o.draggable) holder.style.pointerEvents = "none";
        const marker = new ml.Marker({
          element: holder,
          anchor: "top-left",
          offset: [-o.anchor[0], -o.anchor[1]],
          draggable: !!o.draggable,
        })
          .setLngLat(lngLat(lat, lon))
          .addTo(map);
        // What hovering it says, over it.
        let tip = null;
        if (o.tooltip) {
          holder.addEventListener("mouseenter", () => {
            const at = marker.getLngLat();
            tip = new ml.Popup({
              className: "mc-tip",
              closeButton: false,
              closeOnClick: false,
              focusAfterOpen: false,
              anchor: "bottom",
              maxWidth: "none",
              offset: o.tooltipAnchor ? -o.tooltipAnchor[1] + 4 : 8,
            })
              .setLngLat(at)
              .setDOMContent(o.tooltip)
              .addTo(map);
          });
          holder.addEventListener("mouseleave", () => {
            if (tip) tip.remove();
            tip = null;
          });
        }
        if (o.moved) {
          marker.on("dragend", () => {
            const at = marker.getLngLat().wrap();
            o.moved(at.lat, at.lng);
          });
        }
        if (o.interactive !== false && !o.draggable) {
          holder.addEventListener("click", (event) => {
            event.stopPropagation();
            markerClicked();
            if (o.click) {
              o.click();
              return;
            }
            const box = map.getContainer().getBoundingClientRect();
            clickMap(event.clientX - box.left, event.clientY - box.top, event);
          });
        }
        const handle = {
          move: (lat, lon) => marker.setLngLat(lngLat(lat, lon)),
          remove: () => {
            if (tip) tip.remove();
            marker.remove();
          },
        };
        markers.push(handle);
        return handle;
      },
      line: (points, style) => {
        const shapeId = `${id}-${count++}`;
        keep({
          id: shapeId,
          data: {
            type: "Feature",
            geometry: { type: "LineString", coordinates: points.map(([lat, lon]) => [lon, lat]) },
          },
          layers: [lineLayer(`${shapeId}-line`, style)],
        });
      },
      ring: (lat, lon, pixels, style) => {
        const shapeId = `${id}-${count++}`;
        keep({
          id: shapeId,
          data: { type: "Feature", geometry: { type: "Point", coordinates: lngLat(lat, lon) } },
          layers: [
            {
              id: `${shapeId}-ring`,
              type: "circle",
              paint: {
                "circle-radius": pixels,
                "circle-color": style.fill || "#000000",
                "circle-opacity": style.fill ? (style.fillOpacity ?? 0.2) : 0,
                "circle-stroke-color": style.color,
                "circle-stroke-width": style.weight ?? 2,
                "circle-stroke-opacity": style.opacity ?? 1,
              },
            },
          ],
        });
      },
      circle: (lat, lon, metres, style) => area(circlePolygon(lat, lon, metres), style),
      rect: ([[south, west], [north, east]], style) =>
        area(
          [
            [west, south],
            [east, south],
            [east, north],
            [west, north],
            [west, south],
          ],
          style,
        ),
      clear: () => {
        shapes.forEach(removeShape);
        shapes = [];
        markers.forEach((m) => m.remove());
        markers = [];
      },
      remove: () => {
        shapes.forEach(removeShape);
        shapes = [];
        markers.forEach((m) => m.remove());
        markers = [];
      },
    };
  }

  return { ready, create };
})();
