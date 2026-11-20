// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

// The one map of the interface, drawn by Leaflet, and all the interface asks
// of it: to open on an element, to show the nodes it is given, and to say
// which one was picked. The Rust side calls these as meshctlMap.open and so
// on; nothing else here is reached from outside. What it shows, words and
// all, comes from there: here it is only drawn.
window.meshctlMap = (() => {
  // OpenStreetMap's tiles, as their usage policy asks to be credited.
  const TILES = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
  const ATTRIBUTION =
    '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>';

  // Nodes this close (in pixels) to a click count as clicked: the smallest
  // pins are 20px wide, so those that touch or overlap the one under the
  // pointer join it. A finger covers more, so touch reaches further.
  const PICK_RADIUS = 16;
  const PICK_RADIUS_TOUCH = 26;

  // How long the map stays still before where it is left is kept, in ms.
  const SAVE_AFTER = 1000;

  let map = null;
  let markers = null;
  // What the Rust side gave last: the nodes, and the words to show them with.
  let shown = { labels: {}, nodes: [] };
  // Told when a node is picked (its number) or let go (null), and when one of
  // the node's buttons is pressed (its number and what for).
  let pick = null;
  let act = null;
  // The popup on the picked node, while open, and which node that is; and the
  // ring on the node an item of the list stands for.
  let cardPopup = null;
  let popupFor = null;
  let preview = null;
  // The route drawn, if any.
  let routeLayer = null;
  // The traceroute last drawn, so that the view goes to a route once, not at
  // every redrawing; and whether the picked node's popup is put away for it.
  let routeShown = null;
  // The route drawn, as the Rust side gave it, to draw again at each zoom.
  let shownRoute = null;
  let cardAway = false;

  // Picks a node: its popup shows again, even when it was put away for a
  // route and the node is the one picked already.
  function choose(num) {
    cardAway = false;
    const picked = shown.nodes.find((n) => n.selected);
    if (picked && picked.num === num) {
      showCard();
    } else if (pick) {
      pick(num);
    }
  }
  // Whether the view was already placed: after that it stays where the
  // person leaves it, however the nodes move.
  let placed = false;
  // Where the map was left for the radio: undefined until it is read, null
  // when it never was moved.
  let savedView;
  // Whether the person moved the map: only then is where it is kept.
  let touched = false;
  // Told where the map is left, a while after it stops moving.
  let moved = null;
  let saveTimer = null;
  let allButton = null;

  // A colour of the interface's, as the style sheet has it now.
  const color = (name) =>
    getComputedStyle(document.body).getPropertyValue(name).trim();

  // An element with a class and, if given, its text.
  function element(tag, className, text) {
    const made = document.createElement(tag);
    if (className) made.className = className;
    if (text !== undefined) made.textContent = text;
    return made;
  }

  function open(target, onPick, onAct, onMoved) {
    close();
    map = L.map(target, { zoomControl: true, worldCopyJump: true });
    allControl().addTo(map);
    moved = onMoved;
    savedView = undefined;
    touched = false;
    // The person moving the map, not the map moving itself: from then on,
    // where it is left is kept, and a view read late does not move it.
    for (const kind of ["pointerdown", "wheel", "keydown"]) {
      target.addEventListener(kind, () => (touched = true), { passive: true });
    }
    map.on("moveend", () => {
      if (!touched) return;
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        if (!map || !moved) return;
        const centre = map.getCenter().wrap();
        moved(centre.lat, centre.lng, map.getZoom());
      }, SAVE_AFTER);
    });
    map.attributionControl.setPrefix(false);
    L.tileLayer(TILES, { maxZoom: 19, attribution: ATTRIBUTION }).addTo(map);
    markers = L.layerGroup().addTo(map);
    pick = onPick;
    act = onAct;
    placed = false;
    map.setView([20, 0], 2);
    // A click off every node lets the picked one go.
    map.on("click", (event) => {
      if (!pickAt(event.containerPoint, event.originalEvent)) {
        pick && pick(null);
      }
    });
    // Escape on the map, which Leaflet keeps from reaching the page, takes the
    // route away as it does elsewhere.
    // A route keeps its sides a few pixels apart, whatever the zoom.
    map.on("zoomend", drawRoute);
    // The ring of a node in the overlap list goes as the map zooms: the list
    // is gone by then, and the ring would grow with the zoom's animation.
    map.on("zoomstart", () => showPreview(null));
    map.on("keydown", (event) => {
      if (event.originalEvent.key === "Escape") {
        act && act(0, "unroute");
      }
    });
    // The node's popup closed by the person, not by another opening in its
    // place: the node is let go.
    map.on("popupclose", (event) => {
      if (event.popup === cardPopup) {
        cardPopup = null;
        popupFor = null;
        pick && pick(null);
      }
      showPreview(null);
    });
  }

  function close() {
    if (map) map.remove();
    map = null;
    markers = null;
    pick = null;
    act = null;
    cardPopup = null;
    popupFor = null;
    preview = null;
    routeLayer = null;
    shownRoute = null;
  }

  // The nodes near a point of the map's, the nearest first.
  function nodesNear(point, radius) {
    return visible()
      .map((node) => ({
        node,
        distance: map.latLngToContainerPoint([node.lat, node.lon]).distanceTo(point),
      }))
      .filter(({ distance }) => distance <= radius)
      .sort((a, b) => a.distance - b.distance)
      .map(({ node }) => node);
  }

  // Picks the node under a click, or offers a list when several overlap
  // there. False when there is none within reach.
  function pickAt(point, event) {
    const touch = event && event.pointerType
      ? event.pointerType !== "mouse"
      : matchMedia("(pointer: coarse)").matches;
    const found = nodesNear(point, touch ? PICK_RADIUS_TOUCH : PICK_RADIUS);
    if (found.length === 0) return false;
    if (found.length === 1) {
      choose(found[0].num);
    } else {
      showPicker(found, map.containerPointToLatLng(point));
    }
    return true;
  }

  // A ring on the node an item of the list stands for.
  function showPreview(node) {
    if (preview) preview.remove();
    preview = null;
    if (!node || !map) return;
    preview = L.circleMarker([node.lat, node.lon], {
      radius: 12,
      color: color("--mc-primary"),
      weight: 3,
      fill: false,
      interactive: false,
    }).addTo(map);
  }

  // A node's colour, as the style sheet gives it to its kind.
  function dot(node) {
    return element("span", `mc-dot ${node.kind}`);
  }

  // How big a node's pin is: bigger with an icon, for what its role is, and
  // for the radio's own.
  function pinSize(node) {
    if (node.role === "router") return 32;
    if (node.role) return 28;
    return node.kind === "local" ? 24 : 20;
  }

  // What hovering a node says: who it is, and how it is heard.
  function tip(node) {
    const box = element("div");
    const title = element("div", "mc-tip-title");
    if (node.short) title.append(element("strong", "", node.short));
    title.append(element("span", "", node.name));
    box.append(title);
    if (node.heard) box.append(element("div", "mc-tip-heard", node.heard));
    return box;
  }

  // A node's pin: a dot of its kind's colour, with its role's icon on it.
  function pin(node) {
    const size = pinSize(node);
    const classes = ["mc-pin", node.kind];
    if (node.role) classes.push(`role-${node.role}`);
    if (node.selected) classes.push("selected");
    return L.divIcon({
      className: "",
      html: `<span class="${classes.join(" ")}"><i></i></span>`,
      iconSize: [size, size],
      iconAnchor: [size / 2, size / 2],
      tooltipAnchor: [0, -size / 2],
    });
  }

  // The list of the nodes that overlap where a click fell.
  function showPicker(found, latlng) {
    const labels = shown.labels;
    const box = element("div", "mc-picker");
    const head = element("div", "mc-picker-head");
    head.append(element("p", "", labels.here.replace("{n}", found.length)));
    // Zooming in on the group separates its dots, handier than a long list.
    const zoom = element("button", "mc-picker-zoom");
    zoom.append(element("i"));
    zoom.type = "button";
    zoom.title = labels.zoom;
    zoom.setAttribute("aria-label", labels.zoom);
    zoom.addEventListener("click", (event) => {
      L.DomEvent.stop(event);
      showPreview(null);
      map.closePopup();
      const bounds = L.latLngBounds(found.map((n) => [n.lat, n.lon]));
      // Quick: the group is near, there is no need to fly far.
      map.flyToBounds(bounds, { padding: [60, 60], maxZoom: 18, duration: 0.5 });
    });
    head.append(zoom);
    const list = element("ul", "mc-picker-list");
    for (const node of found) {
      const item = element("li");
      const button = element("button", "mc-picker-item");
      button.type = "button";
      const name = element("span", "mc-picker-name");
      if (node.short) name.append(element("strong", "", node.short));
      name.append(element("span", "", node.name));
      button.append(dot(node), name, element("span", "mc-picker-seen", node.seen || ""));
      button.addEventListener("mouseenter", () => showPreview(node));
      button.addEventListener("focus", () => showPreview(node));
      button.addEventListener("mouseleave", () => showPreview(null));
      button.addEventListener("click", (event) => {
        L.DomEvent.stop(event);
        map.closePopup();
        choose(node.num);
      });
      item.append(button);
      list.append(item);
    }
    box.append(head, list);
    // No cross: a click anywhere else closes it, as picking a node does.
    L.popup({
      className: "mc-picker-popup",
      maxWidth: 300,
      autoPanPadding: [24, 24],
      closeButton: false,
    })
      .setLatLng(latlng)
      .setContent(box)
      .openOn(map);
  }

  // What the popup of a node says, and its buttons.
  function card(node) {
    const labels = shown.labels;
    const box = element("div", "mc-card");
    const head = element("div", "mc-card-head");
    head.append(dot(node));
    const title = element("div", "mc-card-title");
    title.append(element("strong", "", node.name));
    title.append(element("span", "mc-card-id", node.short ? `${node.short} · ${node.id}` : node.id));
    head.append(title);
    const facts = element("dl", "mc-card-facts");
    for (const fact of node.facts) {
      const pair = element("div", fact.wide ? "wide" : "");
      const term = element("dt");
      // One of the bundled Tabler icons, by its name.
      const icon = element("span", "mc-icon");
      icon.style.setProperty(
        "--icon",
        `url(/tabler-icons-3.51.0/icons/outline/${fact.icon}.svg)`,
      );
      icon.setAttribute("aria-hidden", "true");
      term.append(icon, fact.label);
      pair.append(term, element("dd", "", fact.value));
      facts.append(pair);
    }
    const buttons = element("div", "mc-card-buttons");
    for (const [what, label, primary] of [
      ["page", labels.page, false],
      ["chat", labels.chat, true],
    ]) {
      const button = element("button", primary ? "primary" : "", label);
      button.type = "button";
      button.addEventListener("click", (event) => {
        L.DomEvent.stop(event);
        act && act(node.num, what);
      });
      buttons.append(button);
    }
    box.append(head, facts, buttons);
    // Its route: to show, to hide, to trace, or being traced.
    if (node.route) {
      const button = element("button", "mc-card-route", labels[node.route]);
      button.type = "button";
      button.disabled = node.route === "tracing";
      button.setAttribute("aria-pressed", String(node.route === "hide"));
      button.addEventListener("click", (event) => {
        L.DomEvent.stop(event);
        act && act(node.num, "route");
      });
      box.append(button);
    }
    return box;
  }

  // The popup on the picked node, open, current, or closed if there is none.
  function showCard() {
    const picked = shown.nodes.find((n) => n.selected);
    // None picked, or its popup put away for its route: no popup.
    if (!picked || cardAway) {
      if (cardPopup) {
        const open = cardPopup;
        cardPopup = null;
        popupFor = null;
        map.closePopup(open);
      }
      return;
    }
    const content = card(picked);
    if (cardPopup && popupFor === picked.num) {
      cardPopup.setLatLng([picked.lat, picked.lon]).setContent(content);
      return;
    }
    // Whatever popup was open closes as this one opens, and that is no
    // letting go of anything.
    cardPopup = null;
    const popup = L.popup({
      className: "mc-node-popup",
      maxWidth: 320,
      offset: [0, -6],
      autoPanPadding: [24, 24],
    })
      .setLatLng([picked.lat, picked.lon])
      .setContent(content)
      .openOn(map);
    cardPopup = popup;
    popupFor = picked.num;
  }

  // What to show, as JSON: {labels: {page, chat, here, zoom}, nodes: [{num,
  // id, name, short, seen, facts: [[label, value]], lat, lon, kind,
  // selected}]}, kind one of local, direct, near and far. The first time
  // there are nodes, the view goes to the radio's own, or to them all.
  // The nodes on the map: while a route is shown, only those on it, and the
  // one picked, so that the route reads clear of the rest.
  function visible() {
    if (!shownRoute || !shownRoute.nodes) return shown.nodes;
    const on = new Set(shownRoute.nodes);
    return shown.nodes.filter((node) => on.has(node.num) || node.selected);
  }

  function nodes(json) {
    if (!map) return;
    shown = JSON.parse(json);
    const list = shown.nodes;
    drawMarkers();
    labelAll();
    // The first time: where it was left, if that is known already; else to
    // the nodes, until it is.
    if (!placed && list.length > 0) {
      if (savedView) {
        placed = true;
        map.setView([savedView.latitude, savedView.longitude], savedView.zoom);
      } else {
        placeAll();
      }
    }
    showCard();
  }

  function drawMarkers() {
    markers.clearLayers();
    for (const node of visible()) {
      const marker = L.marker([node.lat, node.lon], {
        icon: pin(node),
        keyboard: false,
        // The picked node and the radio's own over the rest.
        zIndexOffset: node.selected ? 1000 : node.kind === "local" ? 500 : node.role ? 100 : 0,
        // Clicks reach the map, which looks for every node near them.
        bubblingMouseEvents: true,
      });
      marker.bindTooltip(tip(node), { direction: "top", className: "mc-tip" });
      marker.addTo(markers);
    }
  }

  const QUALITY_COLOR = {
    good: "--mc-route-good",
    fair: "--mc-route-fair",
    poor: "--mc-route-poor",
    unknown: "--mc-route-unknown",
  };

  // How far apart, in pixels, the way there and the way back run where they
  // go between the same nodes: each keeps to its right, as on a road.
  const ROUTE_SIDE = 4;

  // A stretch moved to the right of the way it goes, in pixels on the screen
  // as the map is now; with the angle it goes at, in degrees.
  function keepRight(from, to) {
    const a = map.latLngToLayerPoint(from);
    const b = map.latLngToLayerPoint(to);
    const length = a.distanceTo(b) || 1;
    // The right of the way from a to b, on a screen whose y goes down.
    const side = L.point(-(b.y - a.y) / length, (b.x - a.x) / length).multiplyBy(ROUTE_SIDE);
    return {
      from: map.layerPointToLatLng(a.add(side)),
      to: map.layerPointToLatLng(b.add(side)),
      angle: (Math.atan2(b.y - a.y, b.x - a.x) * 180) / Math.PI,
    };
  }

  // An arrowhead halfway along a stretch, pointing the way it went.
  function arrow(stretch, fill, casing) {
    const middle = L.latLng(
      (stretch.from.lat + stretch.to.lat) / 2,
      (stretch.from.lng + stretch.to.lng) / 2,
    );
    return L.marker(middle, {
      icon: L.divIcon({
        className: "",
        // Edged like the line it is on, to stand out on the map.
        html:
          `<svg class="mc-arrow" viewBox="0 0 22 22" style="transform:rotate(${stretch.angle}deg)">` +
          `<path d="M4 4 L19 11 L4 18 L8.5 11 Z" fill="${fill}" stroke="${casing}" stroke-width="2.5" stroke-linejoin="round" paint-order="stroke"/>` +
          `</svg>`,
        iconSize: [22, 22],
        iconAnchor: [11, 11],
      }),
      interactive: false,
      keyboard: false,
    });
  }

  // Draws the route shown again: at every zoom, as the sides it keeps to are
  // a few pixels, whatever the scale.
  function drawRoute() {
    if (routeLayer) routeLayer.remove();
    routeLayer = null;
    if (!map || !shownRoute) return;
    routeLayer = L.layerGroup();
    // A wider band under each line, of the colour that stands apart from the
    // tiles (light on a dark map, dark on a light one), as road maps edge
    // their roads: the route reads on any terrain.
    const casing = color("--mc-route-casing");
    const draw = (segment, back) => {
      const stretch = keepRight(segment.from, segment.to);
      const fill = color(QUALITY_COLOR[segment.quality] || "--mc-route-unknown");
      L.polyline([stretch.from, stretch.to], {
        color: casing,
        weight: 7,
        opacity: 0.85,
        lineCap: "round",
        interactive: false,
      }).addTo(routeLayer);
      L.polyline([stretch.from, stretch.to], {
        // The colour says how the hop was heard: grey across nodes that
        // could not be placed, as then it cannot be told.
        color: fill,
        weight: 3,
        // There, solid; back, dashed and a little lighter: always, as the
        // notice's key says.
        opacity: back ? 0.8 : 0.95,
        dashArray: back ? "8 6" : null,
        interactive: false,
      }).addTo(routeLayer);
      arrow(stretch, fill, casing).addTo(routeLayer);
    };
    shownRoute.towards.forEach((segment) => draw(segment, false));
    shownRoute.back.forEach((segment) => draw(segment, true));
    for (const relay of shownRoute.relays) {
      L.circleMarker(relay, {
        radius: 11,
        color: color("--mc-text"),
        weight: 2,
        fill: false,
        interactive: false,
      }).addTo(routeLayer);
    }
    // Lines go in Leaflet's overlay pane, under the markers' own: the nodes
    // stay on top, and can still be picked.
    routeLayer.addTo(map);
  }

  // A traced route, as JSON: {towards: [segment], back: [segment], relays:
  // [[lat, lon]]}, each segment {from: [lat, lon], to: [lat, lon], quality,
  // gap}, gap when nodes between its ends could not be placed. Or null, to
  // take the route off the map.
  function route(json) {
    if (!map) return;
    if (routeLayer) routeLayer.remove();
    routeLayer = null;
    const drawn = json ? JSON.parse(json) : null;
    if (!drawn) {
      routeShown = null;
      if (shownRoute) {
        shownRoute = null;
        drawMarkers();
      }
      return;
    }
    shownRoute = drawn;
    drawMarkers();
    drawRoute();
    // A route shown for the first time: the popup goes out of its way, and
    // the view takes it all in, at once, clear of the notice above and the
    // legend below. Clicking the node brings the popup back.
    if (drawn.id !== routeShown) {
      routeShown = drawn.id;
      cardAway = true;
      showCard();
      const points = [...drawn.towards, ...drawn.back].flatMap((s) => [s.from, s.to]);
      if (points.length > 0) {
        map.fitBounds(points, {
          paddingTopLeft: [40, 80],
          paddingBottomRight: [40, 80],
          maxZoom: 15,
        });
      }
    }
  }

  // Other nodes, of another radio: the view goes to them, as the first time.
  function reset() {
    placed = false;
    savedView = undefined;
    touched = false;
  }

  // Where the map was left for the radio, as JSON {latitude, longitude,
  // zoom}, or null if it never was: the map goes there, or to the nodes,
  // unless the person has moved it already.
  function view(json) {
    savedView = json ? JSON.parse(json) : null;
    if (!map || touched) return;
    if (savedView) {
      map.setView([savedView.latitude, savedView.longitude], savedView.zoom);
      placed = true;
    } else if (!placed) {
      placeAll();
    }
  }

  // Every node in sight: those on the route, while one is shown.
  function fitAll() {
    const list = visible();
    if (list.length === 0) return;
    map.fitBounds(list.map((n) => [n.lat, n.lon]), { maxZoom: 15, padding: [32, 32] });
  }

  // The first view of a radio's nodes: its own node, or all of them.
  function placeAll() {
    const list = shown.nodes;
    if (list.length === 0) return;
    placed = true;
    const local = list.find((n) => n.kind === "local");
    if (local) {
      map.setView([local.lat, local.lon], 11);
    } else {
      map.fitBounds(list.map((n) => [n.lat, n.lon]), { maxZoom: 13, padding: [32, 32] });
    }
  }

  // The button under the zoom's that takes in every node.
  function allControl() {
    const Control = L.Control.extend({
      options: { position: "topleft" },
      onAdd() {
        const bar = L.DomUtil.create("div", "leaflet-bar mc-zoom-all");
        const button = L.DomUtil.create("a", "", bar);
        button.href = "#";
        button.setAttribute("role", "button");
        allButton = button;
        labelAll();
        L.DomEvent.disableClickPropagation(bar);
        L.DomEvent.on(button, "click", (event) => {
          L.DomEvent.preventDefault(event);
          touched = true;
          fitAll();
        });
        return bar;
      },
    });
    return new Control();
  }

  function labelAll() {
    if (!allButton || !shown.labels.all) return;
    allButton.title = shown.labels.all;
    allButton.setAttribute("aria-label", shown.labels.all);
  }

  // The element changed size: the map has to know.
  function resize() {
    if (map) map.invalidateSize();
  }

  return { open, close, nodes, route, reset, resize, view };
})();
