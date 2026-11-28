# meshctl-fakes

Pretend radios, to work on meshctl without one, or without risking one: a
Meshtastic radio and a MeshCore companion radio, each reached over TCP as a
real one on the network is. They transmit nothing, anywhere. They are never
released; they live here, beside what they test, and carry meshctl's
version.

Each plays the firmware as meshctl's code reads it, from the firmware's own
source: they find out what meshctl gets wrong about its own reading, not
what that reading gets wrong about the radios. Real radios still have the
last word.

## Running them

```sh
cargo run -p meshctl-fakes --bin fake-meshtastic -- [port] [control port]
cargo run -p meshctl-fakes --bin fake-meshcore -- [port] [control port]
```

The port is the one clients reach the radio on, on every address: 4403 for
the Meshtastic radio and 5000 for the MeshCore one if none is given. Add it
to meshctl as any radio on the network, by its address and that port.

With a control port, the radio also listens there, on this machine only,
for lines that play what another app or the mesh would do to it. Any client
that writes lines will do:

```sh
printf 'hear 0 hello\n' | nc 127.0.0.1 7435
```

Each line is answered with what it did, and printed on the radio's output
with it; a line it does not know is answered with the lines it does.

## fake-meshtastic

Node `!1234abcd`, *Fake radio* (`FAKE`). It sends every part of the
configuration and every module the protobufs know, answers admin requests
with a passkey, and takes in what it is told to change, so that reading it
back shows it.

Its mesh:

- **Fake neighbour** (`NBR`, `!0badcafe`), a router heard directly, answers
  what it is asked for after a few seconds: who it is, where, its device
  telemetry, environment and local stats (it has no power sensors, so
  `NO_RESPONSE` to that), its neighbours, and traceroutes. As a node does,
  it keeps from answering its node info more than once a minute, and its
  position or neighbours once in three.
- **Fake repeater** (`RPT`, `!0dec0de5`), further away, takes no messages.

Administered over the mesh, the neighbour keeps its own list of nodes as
the firmware does: a contact sent to it a favourite but on a Client Base;
a favourite, or anything but a block, only of a node it knows.

As the firmware does, the radio drops positions and telemetry sent within
10 seconds of the last, waypoints within 10 too, traceroutes within 30 and
texts within 2. Admin requests to any node but the neighbour go unanswered:
the radio gives up on them after two seconds.

| Line | What happens |
| --- | --- |
| `hear <slot> <text>` | The neighbour sends that on a channel. |
| `past <slot>` | Messages on a channel heard over the last year and more, in their order, as days for a conversation to show. |
| `chat <slot>` | Messages on a channel that come every way: directly, through some hops of those they could take, through MQTT; and, after a client wrote, a reply to its last text and reactions to it. |
| `dms` | Nodes that say who they are and write to the radio directly. |
| `fill` | Every slot a channel, and the link dropped to have them read. |
| `twin <from> <to>` | The channel in one slot copied into another: the very same channel twice, as another app may leave it. |
| `off <slot>` | The channel in a slot switched off. |
| `crowd <nodes>` | Many nodes heard, up to 1,000, about Madrid: at every distance up to 40 km, one in ten nowhere and one in twelve on the same spot as the one before; of every role and many models; directly, up to seven hops away, not known how far, or through MQTT; heard minutes, days, weeks or more than a month ago. The same for the same number. The link drops to have them read; `crowd 0`, none. |
| `own` | The radio tells its clients its own device telemetry and how it hears the mesh, as it does now and then. |
| `place` | The neighbour says where it is, as when asked. |
| `move <lat> <lon>` | The neighbour somewhere else, as its position says: to see what follows it, as fences that watch it. |
| `status <text>` | The neighbour says that of itself, as its status message; with no text, nothing. |
| `waypoints <slot>` | Waypoints shared on a channel by the neighbour and the repeater, as the apps share them: one open to all, fenced by a circle; one locked to the repeater, fenced by a box; one that expires in an hour. |
| `unwaypoint <id>` | A waypoint, by its id in hex, deleted for everyone by whoever shared it: sent again, expired. |
| `history <hours>` | The neighbour's telemetry and signal over the last so many hours, as heard then, for charts to show. |
| `storeforward` | The repeater heard as a Store & Forward router: its heartbeat, every 15 minutes and the main one, and its statistics, broadcast. |
| `answers always\|sometimes\|never` | How the neighbour answers. Always is within one to six seconds; sometimes, four times in ten not at all and otherwise within up to 26 seconds. |
| `limits on\|off` | Whether the neighbour keeps from answering too often. |
| `admin allow\|deny` | Whether the neighbour takes the radio's key as an admin key. Allowed, it answers admin requests sealed to it with its settings, the radio's own as they were but for who it is, after a few seconds, and takes changes to them, with the passkey it gave out; denied, it says the key is not one of its own, as the firmware does. |
| `firmware old\|new` | Whether the neighbour's firmware is one from before status messages: old, asked for its own as its admin, it answers nothing, as such a firmware does. |
| `drop` | The link to every client drops, which reads it all anew. |

## fake-meshcore

*Solo probando*, a companion radio. It speaks the companion protocol as the
firmware does, keeps its contacts and channels, queues the messages it hears
to be pulled, and acknowledges direct messages sent to it. What it does not
know yet it refuses, as the firmware does a command it lacks.

Its own settings change as the firmware's do, each checked as it checks
them. Its contacts keep their flags, a favourite among them, as written
back to it whole, and each can be read alone by its key. Its clock starts an hour behind, as a radio's does after a power cut,
and goes only forward. Contacts are added, removed and their paths reset;
channels set and emptied, each with its secret, Galicia's #names at first;
its adverts only printed. Its tuning, auto-add, path hash size and default
flood scope are kept as the firmware keeps them. It has a GPS, off at first:
on, it says it is at the Tower of Hercules, whatever it was told, until it
is off again.

Its contacts, around A Coruña:

- **Ana**, a companion. After a direct message to her, the radio says it was
  delivered, and now and then she answers.
- **Sala Coruña**, a room. It takes `admin` as its admin's password and
  `sala` as its guests'; after a login it sends the posts the radio does not
  have, and takes the radio's own only if it was let in to write.
- **Repetidor Monte** and **Repetidor Torre**, repeaters. They take `admin`
  as their admin's password and none as a guest's, and say nothing to any
  other; logged in, their console answers.
- **Sensor Faro**, a sensor.

| Line | What happens |
| --- | --- |
| `hear <slot> [<name>\|]<text>` | Someone writes that on a channel, as MeshCore clients do, `NAME: text`: the one named, or anyone. |
| `dm <text>` | Ana writes to the radio directly. |
| `react <slot> <emoji>` | Someone reacts to the latest said on a channel, as MeshCore One does. |
| `dmreact <emoji>` | Ana reacts to the latest between her and the radio. |
| `post <text>` | Someone posts in Sala Coruña: Ana, or someone the radio does not know, by turns; sent to the radio once logged in. |
| `room missing` | The room is listed but not found to log in to, as a splitter in between may list a contact the radio no longer has. |
| `room strict\|open` | The room says nothing to a wrong password, as one that lets nobody in to read; or lets them in to read again. |
| `advert [chat\|repeater\|room\|sensor]` | A node not known before advertises, a companion if no kind is given, flooded: one copy through Torre and Monte, another through Monte alone. With no way to it known, as the firmware adds it. The radio takes it as the firmware does: only told of, not added, if its kind is not added by itself or it is further than the hops allowed; added if there is room; in place of the contact changed longest ago, never a favourite, if full and allowed to overwrite one, saying that one is gone; and if not, told of, and the radio says it is full. |
| `crowd <contacts>` | Many contacts, about A Coruña, as many as asked for and the radio has room for, up to its 350 in all: six in ten companions, a quarter repeaters, the rest sensors and rooms; one in ten nowhere; a path known to most; heard minutes, days, weeks or more than a month ago. The same for the same number. The link drops to have them read; `crowd 0`, none. |
| `trace` | Someone else's trace heard going by, raw, along Monte, Torre and Monte again. |
| `answers on\|off` | Whether pings come back and requests are answered. |
| `busy <percent>` | The channel that much busier than it was, as the radio's airtime says, up to 95%; `busy 0`, as it was. |
| `echoes on\|off` | Whether repeaters are heard passing on what the radio sends on a channel; off, as when none heard it. |
| `drop` | The link to every client drops, which reads it all anew. |

## How they are made

`src/lib.rs` holds what both share: how a radio is started, its clients and
control port served, the time and chance. Each radio is a binary in
`src/bin`, in modules:

- **fake-meshtastic**: `radio` (what it is set to, and what it hands its
  clients), `neighbour` (who it is and how it answers), `crowd` (many
  nodes), `link` (its clients), `control` (the control port).
- **fake-meshcore**: `radio` (what it has and is set to), `commands` (what
  it answers each command), `frames` (the frames it sends, made, and what is
  read from those it gets), `nodes` (Ana, the room, the repeaters and the
  sensor), `crowd` (many contacts), `link` (its clients), `control` (the
  control port).

## What to know

- **One queue for every client.** The MeshCore radio hands each message it
  queued to whichever client pulls it first, as the firmware does: a second
  meshctl connected to it takes them from the first.
- **The MeshCore radio seals with meshctl's own code.** Its packets are made
  and read with `meshctl_core::meshcore::raw`, the code it tests: a mistake
  there would be made on both sides and go unseen. That code was checked
  against the firmware's `MyMesh.cpp`; a change to it has to be checked
  there too.
