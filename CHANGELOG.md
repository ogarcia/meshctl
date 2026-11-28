# Changelog

What each release brings. Until 1.0, a release may change the interface, the
API and the database; each one carries the database forward, as far as it can.

## 0.10.0 (2026-11-28)

The live packets drawn as the rest of the panel, and each radio's battery.

### Live

- A packet opened shows what it says over its text, as the rest of the
  panel shows it: a text whole, with who wrote it on a MeshCore channel; a
  node's status; a position, a waypoint or a MeshCore advert on a small map,
  a rounded position with the circle of how far off it may be, a waypoint
  by its emoji; what more a position tells, its altitude, speed, heading,
  satellites and source, in boxes; a waypoint's emoji, name, description,
  expiry and lock, or that it deletes one for everyone.
- Telemetry as a node's figures: battery, channel use, airtime and time on;
  every sensor of its environment; each power channel; how the radio sees
  the mesh. Measures of other kinds by their names.
- Who a node info or an advert says sent it, by its avatar, with its id,
  model and role.
- A traceroute's ways there and back, hop by hop with how well each was
  heard, and on a small map; one on its way, as far as it went.
- The neighbours a node hears, as its page lists them.
- Whether what a routing packet answers got there, or why not, in words.
- A Store & Forward server's heartbeat, statistics and history, and what
  its clients ask.
- What a packet answers or replies to, when it is in the list, opens from
  it.
- On a phone, copying a packet takes the drawer's width.

### Battery

- A radio that runs on its battery shows how full it is, by an icon and in
  percent, beside its name in the sidebar's heading, on the phone's radio at
  the top and in its list of radios; in red under 20%. Plugged in, nothing.
- Meshtastic says it itself. MeshCore says only its voltage, when asked:
  meshctl asks as it gets in sync, then every ten minutes, every two once
  low, and reads it in a straight line from 3.0 V to 4.2 V, as the MeshCore
  apps do; at 4.2 V or more it is taken as plugged in.
- A MeshCore radio's *Power*, in its editor: as its battery says, on its
  battery, always shown, or plugged in, never; for boards that read on USB
  a voltage a battery might.

### Fixes

- A conversation opened with what was not read leaves the line where it
  starts in sight, under some of what was read before, rather than under
  the way back to the newest, which hid it.
- A route on a vector map stays together as the map zooms, its way there
  and back a few pixels apart at any zoom, and no longer blinks as the mesh
  tells of its nodes.
- The live packets' words follow the language.

### Documentation

- How a new language goes into the tables of names kept in the code, in
  `names.rs` and the README.
- The guide tells the live packets' drawer, the battery and *Power*.

### Development

- The fake Meshtastic radio's repeater can be heard as a Store & Forward
  router; the fake MeshCore radio's battery set to a voltage.

### Under the hood

- The Android app built with Gradle 9.9.3, and the Windows app with
  `windows-registry` 0.100.

## 0.9.1 (2026-11-27)

- Leaving a page before what it asked for came, as a node's settings read
  over the mesh, which takes a while, no longer hangs the interface until
  it is reloaded.

## 0.9.0 (2026-11-27)

Nodes removed many at once, MeshCore radios full and their favourites, and
more of a node administered over the mesh.

### Nodes

- A third way to remove a node, beside out of the radio and everything:
  *Only from meshctl*, which deletes all meshctl kept of it, as everything
  does, but asks the radio to forget nothing, and reads it again from it as
  it has it. For whatever meshctl kept of a node that is wrong or no longer
  wanted.
- A bin in the list of nodes' heading removes many at once, rather than
  waiting for meshctl to clean up on its own: of those the list shows, as
  searched and filtered, the ones not heard for more than 8 hours, a day,
  3 days, a week or a month, never a favourite; each of the three ways, with
  how many it takes. It goes one node after another, as far as it got
  shown, and stops at will.

### MeshCore

- A full radio, which keeps no more of the nodes it hears, is told of over
  the panel and as a notification, by the app meshctl runs within or the
  browser: once an hour at most, with the way to the settings that let the
  oldest go. A contact it let go to make room for another is no longer on
  the radio here, still known.
- Favourites: the star on a contact's page marks it on the radio, as the
  firmware keeps it, the rest of its flags as they are. A full radio
  allowed to overwrite one never lets a favourite go.
- A contact with no route says how far its latest advert came from, the
  fewest hops of its copies: "No route · advert 2 hops away" on the list of
  nodes and in chats, on the map's hover and popup, and in its pin's colour.
  Never used as a way to it, which the radio learns otherwise.

### Administering a node

- A node's status message, asked of it apart from the rest of its settings
  and changed as the radio's own: one whose firmware has none, which may not
  answer at all, holds the rest up no longer.
- *Nodes*, under *Device*: what the node does with the nodes of its own
  list, not the radio's. One the radio knows, or a contact's link, sent as a
  contact and a favourite (on a Client Base, made one apart once it has the
  contact); or no longer a favourite, ignored or no longer, forgotten. The
  node does not let its list be read: what it has is not shown, only
  whether it acknowledges each change. Never a contact with no key to trust.

### Fixes

- The zoom's buttons on a dark map stay dark once pressed, and dimmed, not
  light grey, as far in or out as the map goes.
- A trace or a request with no answer no longer says none came in two
  minutes: one to a MeshCore node is given up on in seconds.
- Menus no longer close by themselves: a node's, as the node is heard; a
  chat's share menu and a MeshCore channel's region menu, as messages come
  to another radio.
- A MeshCore room's, repeater's or sensor's page no longer crashes a debug
  build of the interface.

### Documentation

- coresplitter named beside tasticsplitter, for MeshCore, wherever sharing
  a radio between clients is told.
- The guide tells each of the above.

### Development

- The fake radios are a crate of their own, `meshctl-fakes`, with a README:
  `fake-meshtastic` and `fake-meshcore`, each in modules. New scenarios: a
  crowd of nodes on either, a MeshCore radio full with adverts of every
  kind, MeshCore favourites, and a Meshtastic neighbour with an old firmware
  or a list of nodes of its own.

## 0.8.0 (2026-11-27)

Meshtastic's waypoints, with their fences, and vector maps.

### Waypoints

- Meshtastic's waypoints, as the official apps and the firmware share them,
  heard on the mesh and kept for each radio: on its map as their emoji, and
  in a popup that tells who shared and changed them, when they expire and
  on which channel.
- Shared from the map's own menu, a right click or a long touch: an emoji,
  a name and a description, a date to expire on, the channel to go on, and
  how many hops, from 1 to as many as the radio's settings say. Locked to
  the radio, or open for anyone to change. Changed and deleted for everyone
  where the radio may, as Android's app allows; or forgotten here alone.
- A waypoint only in meshctl, never sent, on any radio, MeshCore's too: to
  see on the map, and to share later if wanted.
- Fences: a circle of 100 m to 5 km around a waypoint, or a box, the area
  the map shows. Its author says whether a node going in or out is told
  of, of the favourites only maybe. meshctl watches the radio's own fences,
  and another's once asked from its popup: the latest crossings are kept
  and shown there, and told over the panel and as notifications.
- New waypoints shared by others are told of as notifications too, as each
  channel's notifications say.
- A pin shows or hides the waypoints on the map, for each radio.
- Nodes and waypoints on the same spot open together in one list.

### Maps

- Vector maps, OpenFreeMap's, sharp at any zoom, picked in *Appearance*
  beside OpenStreetMap's tiles, which are still the default. They come
  detailed, vivid, or plain in greys, each with a dark version of its own.
  The small maps on a node's page and in chats stay as tiles.
- OpenStreetMap's tiles may be black and white, for the nodes to stand out.
- A browser that cannot draw vector maps, with no WebGL 2, shows the tiles
  instead and says so, with a way to make it for good; the setting kept for
  every other browser stays as it is.
- The clock and the waypoints' pin are behind three dots under the zoom's
  buttons, in the accent while either leaves something out.
- A popup opened by the map's buttons, its legend or a notice moves the map
  to be clear of them.
- *Appearance* has the map's settings together: its theme, its type and
  its colours.

### Interface

- meshctl's icon, in the browser's tab and in notifications, is in the
  panel's accent.
- The API reference, at `/api/docs`, in meshctl's colours under a bar of
  its own, with the way back to the panel, the guide, the website and the
  code, and an icon of its own in a deep red.

### Fixes

- The panel no longer flashes in amber, or in the system's theme, while it
  loads in another accent or theme.
- A Meshtastic node that reads no messages has its chat button greyed out
  on the map's popup too, as on its page.
- MeshCore repeaters and sensors, which drop any text they are sent, are no
  longer offered a chat.

### Documentation

- A guide to using meshctl, on the website, in English and Spanish.

## 0.7.0 (2026-11-26)

Each node in a colour of its own.

### Nodes and chat

- A node's avatar, on the list of nodes, in chats and on its page, is no
  longer coloured by how many hops away it is, but as Meshtastic's apps
  colour it: by its number, kept within a lightness and chroma that read
  well on the light theme and on the dark. A MeshCore contact is coloured
  by its whole name, the same in chats as on the list, so that call signs
  that start alike do not look alike.
- In a chat, who speaks is in that colour too: their name over their
  messages, and a quote of theirs, made dark or light enough to read on the
  bubble. What the radio itself sent, quoted, stays in the accent.
- The map still colours its nodes by how many hops away they are.

### Fixes

- The map's buttons, on a node's popup, no longer warn in the browser's
  console when used.

## 0.6.0 (2026-11-26)

Other Meshtastic nodes administered over the mesh.

### Remote administration

- *Administer*, on a Meshtastic node's page, opens the same settings page
  as the radio's, for the node: its owner and role, its LoRa, its channels,
  every section of its configuration and modules, read from it over the mesh
  and changed on it, sealed to its key. Each section is asked for as it is
  opened, and what the node took is read back.
- Every change asks first; those to its LoRa and its channels say they may
  leave it out of reach. What is on its way says so and holds still; a node
  that does not answer, or does not take the radio's key, says so, with the
  radio's key to give its owner.
- Its fixed position, set as the radio's is, from where it was last heard.
- Reboot, shut down, forget its nodes, or go back to its factory settings:
  for a node administered, and for the radio itself, among its settings'
  sections, as *Actions*.
- The radio's own admin keys, up to three, added from the nodes known or
  written, under *Device*, *Security*.

### Chat

- A trash can in every chat: a channel is emptied, and so is a MeshCore
  room, both staying on the radio; a conversation with a node is deleted.
  Each asked first, and only in meshctl, not on the mesh.

### Map

- A clock under the zoom's buttons picks how long ago a radio's nodes have
  to have been heard to be on its map: all, or within 8 hours, a day, 3
  days, a week or a month. Kept for each radio; the radio's own node is
  always there.

### Fixes

- Adding a radio changes nothing on it: the channels it has silenced are
  taken as meshctl's, rather than undone.
- A heading's menu, on a phone, as wide as its items, and not pushed right.

### Documentation

- What a message's marks say, and how to administer a node, in the README.

## 0.5.0 (2026-11-26)

Sharing in chats, conversations silenced, and where a radio is.

### Chat

- A "+" beside what is written, to share a contact, a channel or a place.
  On MeshCore as its apps write them: a contact as its key, kind and name,
  a channel by its `meshcore://` link, a place as `latitude,longitude`, any
  picked on a map. On Meshtastic, contacts and channels by their links, and
  where the radio is as its position packet, as precisely as the channel
  shares it.
- What is shared shown as what it is: a contact to see or to add to the
  radio, a channel, with its padlock, to see or to add, a place on a small
  map that opens the big one. On Meshtastic, a place written by hand too.
- A conversation silenced from its heading, for good: no notifications,
  its unread count quiet, a crossed bell beside its name. On a Meshtastic
  channel it is the radio's own silence too, set as meshctl has it.
- While the radio is off, what a message is written in says so itself.

### Radios

- Where a radio is, picked on a map, written, or taken from the device
  meshctl runs on: Meshtastic's fixed position, MeshCore's advertised one.
- A MeshCore radio's GPS on or off, where it has one.
- MeshCore's nodes heard but not added by the radio listed apart, and added
  from their row, their page or their chat; those with no route listed too,
  and the filter through repeaters said as it is.
- The MQTT map's precision on a slider, within what the firmware takes.

### The apps

- On Linux and Windows, radios by Bluetooth, as on Android, paired by the
  system first. Not yet tried on a radio: tell how it goes.
- On Android, the system's back goes back, and twice to leave; where the
  phone is asked for only when a radio's position is taken from it.

### Fixes

- A module's switch is seen to flip as it shows after another.
- No companion's note on the radio's own node.

## 0.4.0 (2026-11-25)

Notifications of the messages heard, wherever meshctl runs.

### Notifications

- Of the messages heard where you say: each radio's direct conversations,
  one node's, or a channel, followed wherever it goes on the radio,
  everything said there or only when it mentions the radio, as `@Name` or
  `@[Name]`.
- Never of the radio's own, a reaction, what comes as it catches up or long
  ago, nor of the conversation you have in front of you.
- A page of their own among meshctl's settings: on or off, with the text or
  only who wrote and where, as two samples show; each radio folded, none,
  some or all of it at once.
- In the browser, while the panel is open in a tab, if it lets; on Android
  and on Linux's desktop, by the app, with its window closed too: one for
  each conversation, which opens on it, and goes once it is read. On
  Windows, as best it can.
- Off on their own, on within the apps, for the direct messages.

### The apps

- On Linux and Windows, going on with the window closed is a setting too,
  as on Android: off, closing it quits meshctl.
- The Android app built with Gradle 9.9 and Java 25.

### Under the hood

- Every dependency brought up to date, the cryptography's new generation
  among them, checked against what the firmware seals.

## 0.3.1 (2026-11-25)

- On Windows, a radio plugged in by its cable, as `COM3`, written or picked
  from those plugged in: it was taken for a host on the network.

## 0.3.0 (2026-11-25)

meshctl on Android, and radios picked from a list rather than written.

### The Android app

- meshctl as an app of its own, for ARM64 phones and older 32-bit ones: the
  daemon inside, on the phone alone, as in the desktop app.
- Radios over the network, by USB, with a cable or an OTG adapter, and by
  Bluetooth.
- Listening to them with the app closed, if you say so in its settings,
  with a notification that tells it is.
- `meshcore://` links, and links shared to it from the Meshtastic or
  MeshCore apps, open in it to add what they hold.
- Its own icon, in layers as Android's are and in one colour where icons are
  themed; the system's bars in meshctl's colours.

### Adding radios

- How meshctl reaches a radio picked from cards (on the network, by USB,
  by Bluetooth where there is a way), and the radio itself from those plugged
  in or heard nearby; its path still written by hand where there are paths.
- A radio's connection changed in the same words it was added in.

### The panel

- Five more accents: blue, teal, violet, coral, and gray, for the interface
  in one colour.
- On a phone, switches beside what they are, a shade over what is touched
  instead of the browser's blue box, and dialogs with room to write in while
  the keyboard takes half the screen.
- The welcome, About and the language picker say what fits in an app too.

## 0.2.0 (2026-11-25)

Automatic replies, and the panel easier at hand.

### Automatic replies

- If someone writes this, answer with that: a ping, say, with the hops it
  took and the signal it came with. On both meshes, from the daemon, with
  the panel closed too.
- A reaction with the hops, 🎯 come direct, a text, or both, and another
  text if it came direct; the texts with variables from the message (who
  sent it, its hops, its SNR and RSSI, its channel, the date and time),
  signed with 🤖 unless told not to.
- Each where it is given, on any radio: every direct conversation, one
  node's, or a channel, followed wherever it goes on the radio. A message
  two would answer is answered by the first, in an order of their own.
- Kept in check: never the radio's own, MQTT's, a reaction, a room's post
  or another bot's; nothing heard while catching up or long ago; the same
  node once in five minutes, and six a minute. Quiet, all of them, on a
  radio that hears its channel busier than 25% of the time, or as set.
- Their own page among meshctl's settings, each shown as it will go.

### The panel

- The map comes back to the area it was left at, all of it, on a screen
  of any size: a phone shows what a desktop showed.
- The numbers from one to seven and the kangaroo among the suggested
  emojis, as the mesh tells hops with them.
- Number fields with less and more buttons in meshctl's look, again and
  again while held.
- Rows dragged to another place show where they will fall, the others
  making way.
- Many radios on the rail, in a short window, scroll without a bar, with
  meshctl's settings always at its foot.

## 0.1.0 (2026-11-24)

The first release: meshctl as it is, to be tried.

### Radios

- Meshtastic and MeshCore radios, as many as you have, over the network or a
  cable, added, ordered, hidden and changed from the panel; each with its own
  colour and short name on the rail.
- Connected around the clock, and again on their own when the link drops,
  what they hear kept in SQLite, for as long as you say.

### The mesh

- Every node heard, when, how well and how far, on a list and on a map; its
  page with what it says about itself, its position, and what it can be asked
  for: status, telemetry, neighbours, position, its owner, a sensor's readings
  over a time, a room's or a repeater's access, a ping.
- Traceroutes on both meshes, and MeshCore's route discovery, with their
  history; every hop on the map, those that are only deduced marked as such.
- The live view: every packet the radio hears, as it hears it, with the route
  it took.

### Chat

- Channels and direct messages, with their history kept while you were away;
  replies, reactions and resending, and one language of marks for what
  happened to each message, on both meshes.
- MeshCore's rooms, logged into with or without a password.

### Settings

- A Meshtastic radio's user, LoRa, channels, device and modules; a MeshCore
  radio's own settings, channels and regions.
- Nodes, channels and a radio's LoRa added from their links, pasted, scanned
  as a QR code with the camera, or read from a photo.
- meshctl's own: the radios, what it keeps and for how long, and how it looks
  (light or dark, and its accent), kept in its database.

### Running it

- `meshctld`, one binary with the interface inside, for Linux (x86_64,
  aarch64) and macOS; a container image, for amd64 and arm64.
- A desktop app for Windows (portable) and Linux (Flatpak), that runs the
  daemon within itself, keeps listening with its window closed, and opens
  `meshcore://` links.
- The panel installs as an app on a phone, and takes links shared to it.
- An access token, which a proxy in front can add to every request; an API
  that every client uses, documented at `/api/docs`.
- In English and Spanish.
