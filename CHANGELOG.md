# Changelog

What each release brings. Until 1.0, a release may change the interface, the
API and the database; each one carries the database forward, as far as it can.

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
