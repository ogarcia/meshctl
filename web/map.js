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

  // Nodes this close (in pixels) to a click count as clicked: the dots are
  // 12px wide, so those that touch or overlap the one under the pointer join
  // it. A finger covers more, so touch reaches further.
  const PICK_RADIUS = 12;
  const PICK_RADIUS_TOUCH = 22;

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
  // Whether the view was already placed: after that it stays where the
  // person leaves it, however the nodes move.
  let placed = false;

  // A colour of the interface's, as the style sheet has it now.
  const color = (name) =>
    getComputedStyle(document.body).getPropertyValue(name).trim();

  const KIND_COLOR = {
    local: "--mc-primary",
    direct: "--mc-second",
    near: "--mc-near",
    far: "--mc-far",
  };

  // An element with a class and, if given, its text.
  function element(tag, className, text) {
    const made = document.createElement(tag);
    if (className) made.className = className;
    if (text !== undefined) made.textContent = text;
    return made;
  }

  function open(target, onPick, onAct) {
    close();
    map = L.map(target, { zoomControl: true, worldCopyJump: true });
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
  }

  // The nodes near a point of the map's, the nearest first.
  function nodesNear(point, radius) {
    return shown.nodes
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
      pick && pick(found[0].num);
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

  function dot(node) {
    const made = element("span", "mc-dot");
    made.style.background = color(KIND_COLOR[node.kind] || "--mc-far");
    return made;
  }

  // The list of the nodes that overlap where a click fell.
  function showPicker(found, latlng) {
    const labels = shown.labels;
    const box = element("div", "mc-picker");
    const head = element("div", "mc-picker-head");
    head.append(element("p", "", labels.here.replace("{n}", found.length)));
    // Zooming in on the group separates its dots, handier than a long list.
    const zoom = element("button", "mc-picker-zoom", "+");
    zoom.type = "button";
    zoom.title = labels.zoom;
    zoom.setAttribute("aria-label", labels.zoom);
    zoom.addEventListener("click", () => {
      map.closePopup();
      const bounds = L.latLngBounds(found.map((n) => [n.lat, n.lon]));
      map.flyToBounds(bounds, { padding: [60, 60], maxZoom: 18 });
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
      button.addEventListener("click", () => {
        map.closePopup();
        pick && pick(node.num);
      });
      item.append(button);
      list.append(item);
    }
    box.append(head, list);
    L.popup({ className: "mc-picker-popup", maxWidth: 300, autoPanPadding: [24, 24] })
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
    for (const [label, value] of node.facts) {
      const pair = element("div");
      pair.append(element("dt", "", label), element("dd", "", value));
      facts.append(pair);
    }
    const buttons = element("div", "mc-card-buttons");
    for (const [what, label, primary] of [
      ["page", labels.page, false],
      ["chat", labels.chat, true],
    ]) {
      const button = element("button", primary ? "primary" : "", label);
      button.type = "button";
      button.addEventListener("click", () => act && act(node.num, what));
      buttons.append(button);
    }
    box.append(head, facts, buttons);
    return box;
  }

  // The popup on the picked node, open, current, or closed if there is none.
  function showCard() {
    const picked = shown.nodes.find((n) => n.selected);
    if (!picked) {
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
  function nodes(json) {
    if (!map) return;
    shown = JSON.parse(json);
    const list = shown.nodes;
    const border = color("--mc-surface");
    markers.clearLayers();
    for (const node of list) {
      const marker = L.circleMarker([node.lat, node.lon], {
        radius: node.kind === "local" || node.selected ? 9 : 6,
        color: node.selected ? color("--mc-text") : border,
        weight: node.selected ? 3 : 2,
        fillColor: color(KIND_COLOR[node.kind] || "--mc-far"),
        fillOpacity: 1,
        // Clicks reach the map, which looks for every node near them.
        bubblingMouseEvents: true,
      });
      const label = node.short ? `${node.short} · ${node.name}` : node.name;
      marker.bindTooltip(label, { direction: "top", offset: [0, -8] });
      marker.addTo(markers);
    }
    if (!placed && list.length > 0) {
      placed = true;
      const local = list.find((n) => n.kind === "local");
      if (local) {
        map.setView([local.lat, local.lon], 11);
      } else {
        map.fitBounds(list.map((n) => [n.lat, n.lon]), { maxZoom: 13, padding: [32, 32] });
      }
    }
    showCard();
  }

  // Other nodes, of another radio: the view goes to them, as the first time.
  function reset() {
    placed = false;
  }

  // The element changed size: the map has to know.
  function resize() {
    if (map) map.invalidateSize();
  }

  return { open, close, nodes, reset, resize };
})();
