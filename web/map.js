// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

// The one map of the interface, and all the interface asks of it: to open on
// an element, to show the nodes it is given, and to say which one was picked.
// The Rust side calls these as meshctlMap.open and so on; nothing else here
// is reached from outside. What it shows, words and all, comes from there:
// here it is only drawn.
//
// What is shown is made here, of the page's own elements: pins, popups,
// lists. Where it is drawn is the engine's, map-leaflet.js, which is the
// only one that knows the library underneath.
window.meshctlMap = (() => {
  const engine = window.meshctlLeaflet;

  // Nodes this close (in pixels) to a click count as clicked: the smallest
  // pins are 20px wide, so those that touch or overlap the one under the
  // pointer join it. A finger covers more, so touch reaches further.
  const PICK_RADIUS = 16;
  const PICK_RADIUS_TOUCH = 26;

  // How long the map stays still before where it is left is kept, in ms.
  const SAVE_AFTER = 1000;
  // How close the map goes to a node shown from its page, at the least.
  const FOCUS_ZOOM = 14;

  let map = null;
  let markers = null;
  // What the Rust side gave last: the nodes, and the words to show them with.
  let shown = { labels: {}, nodes: [], ready: false };
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
  // Where the map was before it went to a route, and the zoom it went to:
  // the route closed with that zoom untouched, the map goes back.
  let beforeRoute = null;

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
  // Told where a waypoint is to be shared, from the map's own menu.
  let onPlaceAt = null;
  // The waypoints, as the Rust side gave them last, their layer, and the
  // popup on one, while open, and which one.
  let shownWaypoints = { labels: {}, waypoints: [], add: false };
  let waypointLayer = null;
  let fenceLayer = null;
  let waypointPopup = null;
  let waypointPopupFor = null;

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

  // A longitude within -180 and 180.
  const wrapLon = (lon) => ((((lon + 180) % 360) + 360) % 360) - 180;

  // The area a circle takes, in metres around a point, as [[south, west],
  // [north, east]].
  function circleBounds(lat, lon, metres) {
    const dLat = metres / 111320;
    const dLon = metres / (111320 * Math.max(Math.cos((lat * Math.PI) / 180), 0.01));
    return [[lat - dLat, lon - dLon], [lat + dLat, lon + dLon]];
  }

  // The pin of a place, its point at its foot, where the place is.
  const placePin = (icon) => ({
    html: `<span class="mc-picked">${icon}</span>`,
    size: [36, 36],
    anchor: [18, 34],
  });

  function open(target, onPick, onAct, onMoved, onPlace) {
    close();
    map = engine.create(target);
    onPlaceAt = onPlace;
    map.control(allControl());
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
      saveTimer = setTimeout(save, SAVE_AFTER);
    });
    fenceLayer = map.layer();
    waypointLayer = map.layer();
    markers = map.layer();
    spotLayer = map.layer();
    preview = map.layer();
    pick = onPick;
    act = onAct;
    placed = false;
    map.setView(20, 0, 2);
    showSpot();
    // A click off every node lets the picked one go.
    map.on("click", (click) => {
      if (!pickAt(click, click.event)) {
        pick && pick(null);
      }
    });
    // A route keeps its sides a few pixels apart, whatever the zoom.
    map.on("zoomend", drawRoute);
    // The ring of a node in the overlap list goes as the map zooms: the list
    // is gone by then, and the ring would grow with the zoom's animation.
    map.on("zoomstart", () => showPreview(null));
    // Escape on the map, which the engine keeps from reaching the page, takes
    // the route away as it does elsewhere.
    map.on("keydown", (event) => {
      if (event.key === "Escape") {
        act && act(0, "unroute");
      }
    });
    // The map's own menu, a right click or a long touch: a waypoint shared
    // there, where the radio can share one.
    map.on("contextmenu", (click) => {
      if (!shownWaypoints.add || !onPlaceAt) return;
      const button = element("button", "mc-place-add", shownWaypoints.labels.add);
      button.type = "button";
      const at = { lat: click.lat, lon: wrapLon(click.lon) };
      button.addEventListener("click", (event) => {
        map.stop(event);
        map.closePopup();
        onPlaceAt(at.lat, at.lon);
      });
      map.popup(click.lat, click.lon, button, {
        className: "mc-place-popup",
        closeButton: false,
        offset: [0, -2],
      });
    });
    // The node's popup closed by the person, not by another opening in its
    // place: the node is let go.
    map.on("popupclose", (popup) => {
      if (popup && popup === waypointPopup) {
        waypointPopup = null;
        waypointPopupFor = null;
      }
      if (popup && popup === cardPopup) {
        cardPopup = null;
        popupFor = null;
        pick && pick(null);
      }
      showPreview(null);
    });
  }

  // A place shown on the map rather than a node, as one shared in a
  // conversation: the map centred there, with a pin, once it is there.
  let spotted = null;
  let spotLayer = null;

  function spot(lat, lon, icon) {
    spotted = { lat, lon, icon };
    showSpot();
  }

  function showSpot() {
    if (!map || !spotted) return;
    const { lat, lon, icon } = spotted;
    spotted = null;
    spotLayer.clear();
    spotLayer.marker(lat, lon, { ...placePin(icon), interactive: false });
    placed = true;
    map.setView(lat, lon, Math.max(map.zoom(), FOCUS_ZOOM), false);
  }

  function close() {
    if (map) map.remove();
    map = null;
    spotLayer = null;
    markers = null;
    waypointLayer = null;
    fenceLayer = null;
    waypointPopup = null;
    waypointPopupFor = null;
    onPlaceAt = null;
    pick = null;
    act = null;
    cardPopup = null;
    popupFor = null;
    preview = null;
    routeLayer = null;
    shownRoute = null;
    beforeRoute = null;
  }

  // The nodes near a point of the map's, the nearest first.
  function nodesNear(point, radius) {
    return visible()
      .map((node) => {
        const at = map.toPoint(node.lat, node.lon);
        return { node, distance: Math.hypot(at.x - point.x, at.y - point.y) };
      })
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
      const at = map.toLatLon(point.x, point.y);
      showPicker(found, at.lat, at.lon);
    }
    return true;
  }

  // A ring on the node an item of the list stands for.
  function showPreview(node) {
    if (!preview) return;
    preview.clear();
    if (!node) return;
    preview.ring(node.lat, node.lon, 12, { color: color("--mc-primary"), weight: 3 });
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
    return {
      html: `<span class="${classes.join(" ")}"><i></i></span>`,
      size: [size, size],
      anchor: [size / 2, size / 2],
      tooltipAnchor: [0, -size / 2],
    };
  }

  // The list of the nodes that overlap where a click fell.
  function showPicker(found, lat, lon) {
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
      map.stop(event);
      showPreview(null);
      map.closePopup();
      // Quick: the group is near, there is no need to fly far.
      map.fit(found.map((n) => [n.lat, n.lon]), {
        padding: 60,
        maxZoom: 18,
        fly: true,
        duration: 0.5,
      });
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
        map.stop(event);
        map.closePopup();
        choose(node.num);
      });
      item.append(button);
      list.append(item);
    }
    box.append(head, list);
    // No cross: a click anywhere else closes it, as picking a node does.
    map.popup(lat, lon, box, {
      className: "mc-picker-popup",
      maxWidth: 300,
      autoPanPadding: [24, 24],
      closeButton: false,
    });
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
      const value = element("dd", fact.clamped ? "clamped" : "", fact.value);
      // Cut at a few lines: whole on hovering.
      if (fact.clamped) value.title = fact.value;
      pair.append(term, value);
      facts.append(pair);
    }
    const buttons = element("div", "mc-card-buttons");
    for (const [what, label, primary] of [
      ["page", labels.page, false],
      ["chat", labels.chat, true],
    ]) {
      // Not written to, the chat is not offered.
      if (what === "chat" && node.writable === false) continue;
      // Written to but read by nobody, it tells: offered, as on its page,
      // with the bubble crossed out and not as the thing to do.
      const unread = what === "chat" && node.unmessagable;
      const button = element("button", primary && !unread ? "primary" : "", label);
      button.type = "button";
      if (unread) {
        const icon = element("span", "mc-icon");
        icon.style.setProperty("--icon", "url(/tabler-icons-3.51.0/icons/outline/message-off.svg)");
        icon.setAttribute("aria-hidden", "true");
        button.prepend(icon);
        button.classList.add("unheeded");
        button.title = labels.unmessagable;
      }
      button.addEventListener("click", (event) => {
        map.stop(event);
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
        map.stop(event);
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
      // Drawn again in place, as it is: neither how far it was scrolled nor
      // where the map was left moves, as setting its content would, and pan
      // the map to it again. Only where the node is, if it moved.
      cardPopup.content().replaceChildren(...content.childNodes);
      const at = cardPopup.at();
      if (at.lat !== picked.lat || at.lon !== picked.lon) {
        cardPopup.move(picked.lat, picked.lon);
      } else {
        cardPopup.reposition();
      }
      return;
    }
    // Whatever popup was open closes as this one opens, and that is no
    // letting go of anything.
    cardPopup = null;
    cardPopup = map.popup(picked.lat, picked.lon, content, {
      className: "mc-node-popup",
      maxWidth: 320,
      // Never taller than the map leaves, as on a phone on its side: what
      // does not fit scrolls within it, the legend under it left clear.
      maxHeight: Math.max(160, map.size().y - 150),
      offset: [0, -6],
      autoPanPadding: [24, 24],
    });
    popupFor = picked.num;
  }

  // What to show, as JSON: {labels: {page, chat, unmessagable, here, zoom},
  // nodes: [{num, id, name, short, seen, facts: [[label, value]], lat, lon,
  // kind, selected, writable, unmessagable}]}, kind one of local, direct,
  // near and far. The first time there are nodes, the view goes to the
  // radio's own, or to them all.
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
    drawMarkers();
    labelAll();
    centre();
    place();
    showCard();
  }

  // The waypoints to show, as JSON: {labels: {add, edit, deleteAll,
  // deleteHere, locked}, add, waypoints: [{id, lat, lon, icon, name,
  // description, facts: [{icon, label, value}], locked, editable, fence:
  // {radius, area: [[south, west], [north, east]]}}]}: add whether the map's
  // menu offers to share one, editable whether this one can be changed and
  // deleted for everyone, fence the circle, in metres, or the box around
  // it, if any.
  function waypoints(json) {
    if (!map) return;
    shownWaypoints = JSON.parse(json);
    drawWaypoints();
    showWaypointCard();
    centreWaypoint();
  }

  // A waypoint's pin: its emoji, on a disc that stands out from the map.
  function waypointPin(waypoint) {
    const pin = element("span", "mc-waypoint");
    pin.append(element("span", "", waypoint.icon));
    return pin.outerHTML;
  }

  function waypointTip(waypoint) {
    const box = element("div");
    const title = element("div", "mc-tip-title");
    title.append(element("span", "", waypoint.name));
    box.append(title);
    if (waypoint.description) box.append(element("div", "mc-tip-heard", waypoint.description));
    return box;
  }

  function drawWaypoints() {
    waypointLayer.clear();
    fenceLayer.clear();
    // Fences in the accent, dashed and faintly filled: an area, not a road.
    const fence = {
      color: color("--mc-primary"),
      weight: 2,
      opacity: 0.8,
      dash: "6 5",
      fill: color("--mc-primary"),
      fillOpacity: 0.07,
    };
    for (const waypoint of shownWaypoints.waypoints) {
      if (waypoint.fence && waypoint.fence.radius) {
        fenceLayer.circle(waypoint.lat, waypoint.lon, waypoint.fence.radius, fence);
      }
      if (waypoint.fence && waypoint.fence.area) {
        fenceLayer.rect(waypoint.fence.area, fence);
      }
    }
    for (const waypoint of shownWaypoints.waypoints) {
      waypointLayer.marker(waypoint.lat, waypoint.lon, {
        html: waypointPin(waypoint),
        size: [30, 30],
        anchor: [15, 15],
        tooltipAnchor: [0, -15],
        tooltip: waypointTip(waypoint),
        // Under the nodes, which are what the map is for.
        z: -100,
        click: () => {
          waypointPopupFor = waypoint.id;
          showWaypointCard(true);
        },
      });
    }
  }

  // What the popup of a waypoint says, and its buttons.
  function waypointCard(waypoint) {
    const labels = shownWaypoints.labels;
    const box = element("div", "mc-card mc-waypoint-card");
    const head = element("div", "mc-card-head");
    head.append(element("span", "mc-waypoint-icon", waypoint.icon));
    const title = element("div", "mc-card-title");
    title.append(element("strong", "", waypoint.name));
    if (waypoint.locked) title.append(element("span", "mc-card-id", labels.locked));
    head.append(title);
    box.append(head);
    if (waypoint.description) {
      box.append(element("p", "mc-waypoint-description", waypoint.description));
    }
    const facts = element("dl", "mc-card-facts");
    for (const fact of waypoint.facts) {
      const pair = element("div", fact.wide ? "wide" : "");
      const term = element("dt");
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
    box.append(facts);
    // The latest nodes gone in or out of its fence.
    if (waypoint.crossings && waypoint.crossings.length) {
      const crossed = element("div", "mc-crossings");
      crossed.append(element("p", "mc-crossings-title", labels.crossings));
      const list = element("ul");
      for (const line of waypoint.crossings) list.append(element("li", "", line));
      crossed.append(list);
      box.append(crossed);
    }
    const buttons = element("div", "mc-card-buttons");
    const actions = waypoint.editable
      ? [["waypoint-edit", labels.edit, true], ["waypoint-delete", labels.deleteAll, false]]
      : [];
    actions.push(["waypoint-forget", labels.deleteHere, false]);
    for (const [what, label, primary] of actions) {
      const button = element("button", primary ? "primary" : "", label);
      button.type = "button";
      button.addEventListener("click", (event) => {
        map.stop(event);
        act && act(waypoint.id, what);
      });
      buttons.append(button);
    }
    box.append(buttons);
    // Its fence told of, or not: as the route button of a node's.
    if (waypoint.watched !== null && waypoint.watched !== undefined) {
      const watch = element(
        "button",
        "mc-card-route",
        waypoint.watched ? labels.unwatch : labels.watch,
      );
      watch.type = "button";
      watch.setAttribute("aria-pressed", String(waypoint.watched));
      watch.addEventListener("click", (event) => {
        map.stop(event);
        act && act(waypoint.id, waypoint.watched ? "waypoint-unwatch" : "waypoint-watch");
      });
      box.append(watch);
    }
    return box;
  }

  // The popup on the waypoint picked, opened (open), current, or closed if
  // it is gone.
  function showWaypointCard(open) {
    if (waypointPopupFor === null) return;
    const waypoint = shownWaypoints.waypoints.find((w) => w.id === waypointPopupFor);
    if (!waypoint) {
      if (waypointPopup) map.closePopup(waypointPopup);
      waypointPopup = null;
      waypointPopupFor = null;
      return;
    }
    const content = waypointCard(waypoint);
    if (waypointPopup && !open) {
      waypointPopup.content().replaceChildren(...content.childNodes);
      waypointPopup.move(waypoint.lat, waypoint.lon);
      return;
    }
    if (!open) return;
    // Whatever was picked is let go: one popup at a time.
    const id = waypointPopupFor;
    if (cardPopup) {
      const shown = cardPopup;
      cardPopup = null;
      popupFor = null;
      map.closePopup(shown);
      pick && pick(null);
    }
    waypointPopupFor = id;
    waypointPopup = map.popup(waypoint.lat, waypoint.lon, content, {
      className: "mc-node-popup",
      maxWidth: 320,
      maxHeight: Math.max(160, map.size().y - 150),
      offset: [0, -12],
      autoPanPadding: [24, 24],
    });
  }

  // A waypoint to show, from its notification: the map goes to it once it
  // has it, with its popup open.
  let focusWaypointOn = null;

  function focusWaypoint(id) {
    focusWaypointOn = id;
    centreWaypoint();
  }

  function centreWaypoint() {
    if (!map || focusWaypointOn === null) return;
    const waypoint = shownWaypoints.waypoints.find((w) => w.id === focusWaypointOn);
    if (!waypoint) return;
    focusWaypointOn = null;
    placed = true;
    map.setView(waypoint.lat, waypoint.lon, Math.max(map.zoom(), FOCUS_ZOOM), false);
    waypointPopupFor = waypoint.id;
    showWaypointCard(true);
  }

  // The area the map shows now, as JSON {south, west, north, east}, within
  // the world; null with no map.
  function bounds() {
    if (!map) return null;
    const b = map.bounds();
    return JSON.stringify({
      south: Math.max(b.south, -90),
      west: Math.max(b.west, -180),
      north: Math.min(b.north, 90),
      east: Math.min(b.east, 180),
    });
  }

  // A node to show, asked for from elsewhere, as its page: the map goes to
  // it once it has it, instead of to where it was left.
  let focusOn = null;

  function focus(num) {
    focusOn = num;
    centre();
  }

  function centre() {
    if (!map || focusOn === null) return;
    const node = shown.nodes.find((n) => n.num === focusOn);
    if (!node) return;
    focusOn = null;
    placed = true;
    map.setView(node.lat, node.lon, Math.max(map.zoom(), FOCUS_ZOOM), false);
    // Its popup, opened before, fitted in again now that it is centred:
    // over it, it could be left above the map. Once, here; never as it is
    // only drawn again.
    if (cardPopup) cardPopup.fit();
  }

  function drawMarkers() {
    markers.clear();
    for (const node of visible()) {
      markers.marker(node.lat, node.lon, {
        ...pin(node),
        tooltip: tip(node),
        // The picked node and the radio's own over the rest.
        z: node.selected ? 1000 : node.kind === "local" ? 500 : node.role ? 100 : 0,
      });
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
  function keepRight(on, from, to) {
    const a = on.toPoint(from[0], from[1]);
    const b = on.toPoint(to[0], to[1]);
    const length = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    // The right of the way from a to b, on a screen whose y goes down.
    const side = { x: (-(b.y - a.y) / length) * ROUTE_SIDE, y: ((b.x - a.x) / length) * ROUTE_SIDE };
    const moved = (p) => {
      const at = on.toLatLon(p.x + side.x, p.y + side.y);
      return [at.lat, at.lon];
    };
    return {
      from: moved(a),
      to: moved(b),
      angle: (Math.atan2(b.y - a.y, b.x - a.x) * 180) / Math.PI,
    };
  }

  // An arrowhead halfway along a stretch, pointing the way it went.
  function arrow(layer, stretch, fill, casing) {
    layer.marker((stretch.from[0] + stretch.to[0]) / 2, (stretch.from[1] + stretch.to[1]) / 2, {
      // Edged like the line it is on, to stand out on the map.
      html:
        `<svg class="mc-arrow" viewBox="0 0 22 22" style="transform:rotate(${stretch.angle}deg)">` +
        `<path d="M4 4 L19 11 L4 18 L8.5 11 Z" fill="${fill}" stroke="${casing}" stroke-width="2.5" stroke-linejoin="round" paint-order="stroke"/>` +
        `</svg>`,
      size: [22, 22],
      anchor: [11, 11],
      interactive: false,
    });
  }

  // Draws the route shown again: at every zoom, as the sides it keeps to are
  // a few pixels, whatever the scale.
  function drawRoute() {
    if (routeLayer) routeLayer.remove();
    routeLayer = null;
    if (!map || !shownRoute) return;
    routeLayer = map.layer();
    // A wider band under each line, of the colour that stands apart from the
    // tiles (light on a dark map, dark on a light one), as road maps edge
    // their roads: the route reads on any terrain.
    const casing = color("--mc-route-casing");
    const draw = (segment, back) => {
      const stretch = keepRight(map, segment.from, segment.to);
      const fill = color(QUALITY_COLOR[segment.quality] || "--mc-route-unknown");
      routeLayer.line([stretch.from, stretch.to], { color: casing, weight: 7, opacity: 0.85 });
      routeLayer.line([stretch.from, stretch.to], {
        // The colour says how the hop was heard: grey across nodes that
        // could not be placed, as then it cannot be told.
        color: fill,
        weight: 3,
        // There, solid; back, dashed and a little lighter: always, as the
        // notice's key says.
        opacity: back ? 0.8 : 0.95,
        dash: back ? "8 6" : null,
      });
      arrow(routeLayer, stretch, fill, casing);
    };
    shownRoute.towards.forEach((segment) => draw(segment, false));
    shownRoute.back.forEach((segment) => draw(segment, true));
    for (const relay of shownRoute.relays) {
      routeLayer.ring(relay[0], relay[1], 11, { color: color("--mc-text"), weight: 2 });
    }
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
      goBack();
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
        // Another route in place of the first: back to before the first.
        if (!beforeRoute) {
          beforeRoute = { centre: map.centre(), zoom: map.zoom() };
        }
        beforeRoute.routeZoom = Math.min(15, map.fitZoom(points, [80, 160]));
        map.fit(points, {
          paddingTopLeft: [40, 80],
          paddingBottomRight: [40, 80],
          maxZoom: 15,
        });
      }
    }
  }

  // The route gone: the map goes back to where it was before it, unless the
  // person zoomed meanwhile, and so chose where to look.
  function goBack() {
    const before = beforeRoute;
    beforeRoute = null;
    if (!before || map.zoom() !== before.routeZoom) return;
    map.setView(before.centre.lat, before.centre.lon, before.zoom);
  }

  // The route about to go for a reason that is not closing it, as another
  // node picked: the map stays where it is.
  function stay() {
    beforeRoute = null;
  }

  // Tells where the map is left, now: the area it shows, to show it all
  // again on any screen. Its centre within -180 and 180 of longitude, and no
  // wider or taller than the world, as one zoomed out shows it.
  function save() {
    saveTimer = null;
    if (!map || !moved) return;
    const bounds = map.bounds();
    const centre = map.centre();
    const shift = wrapLon(centre.lon) - centre.lon;
    const west = bounds.west + shift;
    const east = bounds.east + shift;
    const width = Math.min(east - west, 360);
    const middle = (west + east) / 2;
    moved(
      Math.max(bounds.south, -90),
      middle - width / 2,
      Math.min(bounds.north, 90),
      middle + width / 2,
    );
  }

  // Other nodes, of another radio: the view goes to them, as the first time.
  // Where the last one was left, if not kept yet, is kept now, for it.
  function reset() {
    if (saveTimer) {
      clearTimeout(saveTimer);
      save();
    }
    placed = false;
    savedView = undefined;
    touched = false;
    beforeRoute = null;
  }

  // Where the map was left for the radio, as JSON {south, west, north,
  // east}, or null if it never was.
  function view(json) {
    savedView = json ? JSON.parse(json) : null;
    place();
  }

  // The first view of a radio, once, with both where it was left read and
  // all of its nodes there: where it was left, or else its nodes. Not if the
  // person has moved the map meanwhile. It jumps there: an animation from
  // the last radio's place says nothing. The area left, all of it, at the
  // zoom it fits in on this screen, which may be another than it was left
  // on: a phone's shows less of it at the same zoom.
  function place() {
    if (!map || placed || touched || savedView === undefined || !shown.ready) return;
    if (savedView) {
      placed = true;
      const { south, west, north, east } = savedView;
      map.fit([[south, west], [north, east]], { animate: false });
    } else {
      placeAll();
    }
  }

  // Every node in sight: those on the route, while one is shown.
  function fitAll() {
    const list = visible();
    if (list.length === 0) return;
    map.fit(list.map((n) => [n.lat, n.lon]), { maxZoom: 15, padding: 32 });
  }

  // The first view of a radio's nodes: its own node, or all of them.
  function placeAll() {
    const list = shown.nodes;
    if (list.length === 0) return;
    placed = true;
    const local = list.find((n) => n.kind === "local");
    if (local) {
      map.setView(local.lat, local.lon, 11, false);
    } else {
      map.fit(list.map((n) => [n.lat, n.lon]), { maxZoom: 13, padding: 32, animate: false });
    }
  }

  // The button under the zoom's that takes in every node.
  function allControl() {
    const bar = element("div", "leaflet-bar mc-zoom-all");
    const button = element("a", "");
    bar.append(button);
    button.href = "#";
    button.setAttribute("role", "button");
    allButton = button;
    labelAll();
    button.addEventListener("click", (event) => {
      event.preventDefault();
      touched = true;
      fitAll();
    });
    return bar;
  }

  function labelAll() {
    if (!allButton || !shown.labels.all) return;
    allButton.title = shown.labels.all;
    allButton.setAttribute("aria-label", shown.labels.all);
  }

  // The element changed size: the map has to know.
  function resize() {
    if (map) map.resize();
  }

  // The small maps of the nodes' pages, apart from the one map: each where
  // a node is, still (nothing moves it but the page), with the circle of
  // how far it may be from there when it says. By their elements.
  const minis = new Map();

  // The small map on an element, made the first time: still, its tiles and
  // a layer for what is drawn on it.
  function smallMap(element) {
    let small = minis.get(element);
    if (!small) {
      // In quarters, so that the circle fills it, whatever its size.
      small = engine.create(element, { still: true, zoomSnap: 0.25 });
      small.drawn = small.layer();
      minis.set(element, small);
    }
    return small;
  }

  // Draws a small map on an element, or draws it again: where (lat, lon),
  // how far it may be from there (radius, in metres, or none) and the
  // colour of its pin, as one of the interface's (pin).
  function mini(element, json) {
    const at = JSON.parse(json);
    const small = smallMap(element);
    // Laid out first: placed by its circle, it has to know its size.
    requestAnimationFrame(() => {
      small.resize();
      small.drawn.clear();
      // Placed before the circle is drawn: a circle on a map not yet placed
      // has nothing to be placed by.
      if (at.radius) {
        small.fit(circleBounds(at.lat, at.lon, at.radius), {
          padding: 12,
          maxZoom: 16,
          animate: false,
        });
        const primary = color("--mc-primary");
        small.drawn.circle(at.lat, at.lon, at.radius, {
          color: primary,
          weight: 1.5,
          fill: primary,
          fillOpacity: 0.12,
        });
      } else {
        small.setView(at.lat, at.lon, FOCUS_ZOOM, false);
      }
      small.drawn.ring(at.lat, at.lon, 7, {
        color: color("--mc-surface"),
        weight: 2,
        fill: color(at.pin),
        fillOpacity: 1,
      });
    });
  }

  // The way a message came, on a small map, as JSON: {points: [{lat, lon,
  // pin, gap}]}, in order from its sender's side to the radio, each with
  // the colour of its pin; gap on one reached across repeaters that could
  // not be placed, its stretch dashed.
  function miniRoute(element, json) {
    const drawn = JSON.parse(json);
    const small = smallMap(element);
    requestAnimationFrame(() => {
      small.resize();
      small.drawn.clear();
      const points = drawn.points.map((p) => [p.lat, p.lon]);
      if (points.length === 0) return;
      const casing = color("--mc-route-casing");
      for (let i = 1; i < points.length; i++) {
        const stretch = [points[i - 1], points[i]];
        small.drawn.line(stretch, { color: casing, weight: 6, opacity: 0.85 });
        small.drawn.line(stretch, {
          color: color("--mc-primary"),
          weight: 3,
          dash: drawn.points[i].gap ? "6 6" : null,
        });
      }
      // A guessed one hollow, its ring dashed.
      drawn.points.forEach((p, i) => {
        small.drawn.ring(points[i][0], points[i][1], 6, {
          color: p.guess ? color(p.pin) : color("--mc-surface"),
          weight: 2,
          fill: color(p.pin),
          fillOpacity: p.guess ? 0.25 : 1,
          dash: p.guess ? "3 2" : null,
        });
      });
      if (points.length === 1) {
        small.setView(points[0][0], points[0][1], FOCUS_ZOOM, false);
      } else {
        small.fit(points, { padding: 20, maxZoom: 15, animate: false });
      }
      // An arrowhead halfway along each stretch, the way it went: its angle
      // on the screen, so known once the map is placed.
      const fill = color("--mc-primary");
      for (let i = 1; i < points.length; i++) {
        const from = small.toPoint(points[i - 1][0], points[i - 1][1]);
        const to = small.toPoint(points[i][0], points[i][1]);
        if (Math.hypot(to.x - from.x, to.y - from.y) < 24) continue;
        const angle = (Math.atan2(to.y - from.y, to.x - from.x) * 180) / Math.PI;
        arrow(small.drawn, { from: points[i - 1], to: points[i], angle }, fill, casing);
      }
    });
  }

  // The small map on an element, gone with it.
  function miniClose(element) {
    const small = minis.get(element);
    if (small) {
      small.remove();
      minis.delete(element);
    }
  }

  // Maps to pick a place on, by the element each is drawn on.
  const pickers = new Map();

  // A map to pick a place on, as JSON: {lat, lon, zoom} where it looks
  // first, pin whether a pin starts there, and icon the pin's own drawing.
  // The pin is dragged, or put where the map is clicked, and moved(lat,
  // lon) told where it went.
  function picker(element, json, moved) {
    const at = JSON.parse(json);
    const picking = engine.create(element, { zoomSnap: 0.5 });
    const layer = picking.layer();
    let marker = null;
    const put = (lat, lon) => {
      if (marker) {
        marker.move(lat, lon);
        return;
      }
      marker = layer.marker(lat, lon, { ...placePin(at.icon), draggable: true, moved });
    };
    picking.on("click", (click) => {
      const lon = wrapLon(click.lon);
      put(click.lat, lon);
      moved(click.lat, lon);
    });
    picking.setView(at.lat, at.lon, at.zoom, false);
    if (at.pin) put(at.lat, at.lon);
    pickers.set(element, { map: picking, put });
    // Laid out by then, in a dialog that has just opened.
    requestAnimationFrame(() => picking.resize());
  }

  // Puts the pin of a map to pick on somewhere, and goes there.
  function pickerAt(element, lat, lon) {
    const picking = pickers.get(element);
    if (!picking) return;
    picking.put(lat, lon);
    picking.map.setView(lat, lon, Math.max(picking.map.zoom(), FOCUS_ZOOM + 2));
  }

  function pickerClose(element) {
    const picking = pickers.get(element);
    if (picking) {
      picking.map.remove();
      pickers.delete(element);
    }
  }

  return {
    open, close, nodes, waypoints, focusWaypoint, bounds, focus, route, stay, reset, resize, view, mini, miniRoute,
    miniClose, picker, pickerAt, pickerClose, spot,
  };
})();
