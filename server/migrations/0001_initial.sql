-- meshctl's database, whole. While meshctl is in development this is the only
-- migration: the schema changes here, and a database made with an earlier one
-- is to be removed and made anew.

-- The radios meshctl keeps connected to. Kinds that this build cannot talk to
-- yet are refused by the API, not here, so that adding support for one never
-- needs a migration.
CREATE TABLE radio (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    address TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    -- How it shows on the interface's rail: whether it does, in which place,
    -- and how its button looks, as the JSON of the API's Badge (NULL for the
    -- rail's colour and the radio's initials).
    shown INTEGER NOT NULL DEFAULT 1,
    position INTEGER NOT NULL DEFAULT 0,
    badge TEXT,
    -- Where the interface's map was left for it, as the JSON of the API's
    -- MapView: NULL until it is first moved.
    map_view TEXT,
    -- What it last knew of itself, so that a radio disabled, out of reach or
    -- just starting up still shows it: when it was last connected and in
    -- sync, in seconds since the epoch, and how it described itself then.
    last_connected INTEGER,
    local_num INTEGER,
    local_firmware_version TEXT,
    local_hw_model TEXT,
    local_role TEXT,
    local_region TEXT,
    local_modem_preset TEXT
);

-- The nodes each radio has heard. Removing a radio forgets them, as it does
-- everything else of it.
CREATE TABLE node (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    num INTEGER NOT NULL,
    long_name TEXT,
    short_name TEXT,
    hw_model TEXT,
    role TEXT,
    -- Its public key, 32 bytes, as it sends it; NULL when not known.
    public_key BLOB,
    -- Whether its owner is a licensed radio amateur, who sends nothing
    -- encrypted.
    licensed INTEGER NOT NULL DEFAULT 0,
    -- Whether it tells that nobody reads messages sent to it.
    unmessagable INTEGER NOT NULL DEFAULT 0,
    latitude REAL,
    longitude REAL,
    altitude INTEGER,
    -- How far, in metres, its position may be from where it is, when it
    -- shares it rounded, as most do; NULL when exact or not said.
    position_radius INTEGER,
    -- How it came by its position: manual, internal or external; NULL when
    -- it does not say.
    position_source TEXT,
    last_heard INTEGER,
    snr REAL,
    rssi INTEGER,
    hops_away INTEGER,
    via_mqtt INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    battery_level INTEGER,
    voltage REAL,
    -- How busy it hears the channel, as a percentage of the time.
    channel_utilization REAL,
    PRIMARY KEY (radio_id, num)
);

-- Each radio's channels, as conversations: one is the same channel for as
-- long as it is, whatever slot it is in, and through its name or key being
-- changed where it is. It is told by its fingerprint, a hash of its name
-- and key as the radio uses them, never the key itself; two the very same
-- are one. One in no slot is one taken off the radio: its messages wait for
-- it, should it come back, or to be cleared.
CREATE TABLE channel (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    fingerprint BLOB NOT NULL,
    -- Its name when last seen; empty for one known only by its slot so far.
    name TEXT NOT NULL,
    seen_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX channel_fingerprint ON channel (radio_id, fingerprint);

-- A MeshCore channel's region, which what is sent on it floods in, by the
-- channel's fingerprint: its name, or * for none. The radio keeps none per
-- channel. Apart from the channels, as one may be kept for a channel before
-- the radio has it.
CREATE TABLE channel_scope (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    fingerprint BLOB NOT NULL,
    scope TEXT NOT NULL,
    PRIMARY KEY (radio_id, fingerprint)
);

-- How far each conversation of a radio was read: up to which message, the
-- newest seen. A channel's, wherever the channel is now; or a node's.
CREATE TABLE conversation_read (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    channel_id INTEGER REFERENCES channel (id) ON DELETE CASCADE,
    node INTEGER,
    message_id INTEGER NOT NULL,
    CHECK ((channel_id IS NULL) != (node IS NULL))
);

CREATE UNIQUE INDEX conversation_read_channel ON conversation_read (channel_id)
    WHERE channel_id IS NOT NULL;
CREATE UNIQUE INDEX conversation_read_node ON conversation_read (radio_id, node)
    WHERE node IS NOT NULL;

-- Which channel each slot of a radio has, as the radio was last read.
CREATE TABLE channel_slot (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    slot INTEGER NOT NULL,
    channel_id INTEGER NOT NULL REFERENCES channel (id) ON DELETE CASCADE,
    PRIMARY KEY (radio_id, slot)
);

CREATE INDEX channel_slot_channel ON channel_slot (channel_id);

-- The messages each radio heard or sent.
CREATE TABLE message (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    packet_id INTEGER NOT NULL,
    from_num INTEGER NOT NULL,
    -- The sender's name as the message gives it, for those that say no more:
    -- a MeshCore channel message carries no sender, only the name its client
    -- wrote into the text.
    sender_name TEXT,
    -- 4294967295 for a channel message, a node for a direct one.
    to_num INTEGER NOT NULL,
    -- The slot it was heard or sent on.
    channel INTEGER NOT NULL,
    -- For a channel message, its conversation: which channel it is, wherever
    -- that is now.
    channel_id INTEGER REFERENCES channel (id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    at INTEGER NOT NULL,
    outgoing INTEGER NOT NULL,
    -- received, sending, sent, delivered or failed.
    status TEXT NOT NULL,
    error TEXT,
    reply_id INTEGER,
    emoji INTEGER NOT NULL DEFAULT 0,
    snr REAL,
    rssi INTEGER,
    hops_away INTEGER,
    via_mqtt INTEGER NOT NULL DEFAULT 0,
    -- The region it went flooded in, for one sent scoped to one.
    scope TEXT,
    -- When its sender says it sent it, by its clock: on MeshCore, where
    -- replies and reactions name a message by it.
    sent_at INTEGER,
    -- Who wrote it, for a MeshCore room's post, which comes from the room:
    -- the node its author's key starts with.
    author_num INTEGER
);

-- A packet is heard once: the copies a radio or a tasticsplitter replays on
-- reconnecting are the same message.
CREATE UNIQUE INDEX message_packet ON message (radio_id, from_num, packet_id);
CREATE INDEX message_channel ON message (channel_id, id) WHERE channel_id IS NOT NULL;
CREATE INDEX message_direct ON message (radio_id, to_num, id);

-- Each copy of a message a MeshCore radio heard, and the way it came, by the
-- message's packet id: a copy may come before its message is kept.
CREATE TABLE message_route (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    packet_id INTEGER NOT NULL,
    -- The repeaters' hashes it went through, from its sender's side, each
    -- hash_size bytes.
    path BLOB NOT NULL,
    hash_size INTEGER NOT NULL,
    flooded INTEGER NOT NULL,
    snr REAL,
    rssi INTEGER,
    -- The region it came in, by its name; ? for one whose key is not known.
    scope TEXT,
    at INTEGER NOT NULL
);

CREATE INDEX message_route_packet ON message_route (radio_id, packet_id, id);

-- The repeaters a message was heard through, each once whatever copies went
-- through it: how many carried it.
CREATE TABLE message_relay (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    packet_id INTEGER NOT NULL,
    hop BLOB NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (radio_id, packet_id, hop)
);

-- The messages as shown: a channel message with the slot its channel has
-- now, if any.
CREATE VIEW message_view AS
SELECT
    m.id, m.radio_id, m.packet_id, m.from_num, m.sender_name, m.to_num,
    coalesce(
        (SELECT min(s.slot) FROM channel_slot s WHERE s.channel_id = m.channel_id),
        m.channel
    ) AS channel,
    m.channel_id, m.text, m.at, m.outgoing, m.status, m.error, m.reply_id,
    m.emoji, m.snr, m.rssi, m.hops_away, m.via_mqtt, m.sent_at, m.author_num,
    coalesce(m.scope, (
        SELECT r.scope FROM message_route r
        WHERE r.radio_id = m.radio_id AND r.packet_id = m.packet_id AND r.scope IS NOT NULL
        ORDER BY r.id LIMIT 1
    )) AS scope,
    (
        SELECT count(*) FROM message_route r
        WHERE r.radio_id = m.radio_id AND r.packet_id = m.packet_id
    ) AS heard,
    (
        SELECT count(*) FROM message_relay h
        WHERE h.radio_id = m.radio_id AND h.packet_id = m.packet_id
    ) AS repeaters
FROM message m;

-- The password each radio logs in to a MeshCore room with, kept from the last
-- login the room took: in base64, so as not to be read at a glance.
CREATE TABLE node_password (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    node INTEGER NOT NULL,
    password TEXT NOT NULL,
    PRIMARY KEY (radio_id, node)
);

-- The traceroutes each radio sent.
CREATE TABLE traceroute (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    packet_id INTEGER NOT NULL,
    -- The radio's own node when it was sent, where the route starts.
    origin INTEGER NOT NULL,
    target INTEGER NOT NULL,
    at INTEGER NOT NULL,
    -- pending, done or failed.
    status TEXT NOT NULL,
    error TEXT,
    -- The routes, once answered, as JSON arrays of {node, snr}.
    towards TEXT,
    back TEXT
);

CREATE INDEX traceroute_radio ON traceroute (radio_id, id);
CREATE INDEX traceroute_pending ON traceroute (radio_id, packet_id) WHERE status = 'pending';

-- What is set from the interface and kept for every browser that opens it,
-- one group of settings to a row, as JSON: the interface's look and language
-- under 'interface' today.
CREATE TABLE preference (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

-- Every packet the radios hear, whatever it carries: the log of the mesh, from
-- which what was already seen can be drawn again. A packet is kept once,
-- however many radios hear it; each hearing is a sighting of it, with how
-- that radio heard it.
CREATE TABLE packet (
    id INTEGER PRIMARY KEY,
    -- Its id on the mesh, which with its sender names it.
    packet_id INTEGER NOT NULL,
    from_num INTEGER NOT NULL,
    -- 4294967295 for everyone.
    to_num INTEGER NOT NULL,
    -- As the protobufs number it; NULL while it is encrypted.
    portnum INTEGER,
    -- What it carries, as it travelled: a protobuf for most ports, or still
    -- encrypted.
    payload BLOB NOT NULL,
    request_id INTEGER,
    reply_id INTEGER,
    want_response INTEGER NOT NULL DEFAULT 0,
    pki INTEGER NOT NULL DEFAULT 0,
    -- For telemetry, which kind it carries, as its field in the protobufs
    -- numbers it (2 device, 3 environment…): the latest of each is kept,
    -- however old. NULL for any other port.
    kind INTEGER,
    -- When it was first heard, by any radio.
    at INTEGER NOT NULL
);

CREATE UNIQUE INDEX packet_identity ON packet (from_num, packet_id);
CREATE INDEX packet_to ON packet (to_num, at);
-- What a node sent, by when: its telemetry and signal over a while, and the
-- latest of each kind.
CREATE INDEX packet_from ON packet (from_num, at);
CREATE INDEX packet_request ON packet (request_id) WHERE request_id IS NOT NULL;
-- The oldest heard, to clean away first.
CREATE INDEX packet_at ON packet (at);

-- Each radio's hearing of a packet. A radio, or a tasticsplitter, replays
-- packets on reconnecting: the same radio hears a packet once.
CREATE TABLE packet_seen (
    id INTEGER PRIMARY KEY,
    packet INTEGER NOT NULL REFERENCES packet (id) ON DELETE CASCADE,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    -- The slot it was heard on, which another radio may have another channel
    -- in; and the channel that was, wherever it is now.
    channel INTEGER NOT NULL,
    channel_id INTEGER REFERENCES channel (id) ON DELETE SET NULL,
    at INTEGER NOT NULL,
    snr REAL,
    rssi INTEGER,
    hop_limit INTEGER NOT NULL,
    hop_start INTEGER NOT NULL,
    -- The last byte of the node that relayed it last, when the firmware says.
    relay_node INTEGER,
    via_mqtt INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX packet_seen_once ON packet_seen (packet, radio_id);
CREATE INDEX packet_seen_radio ON packet_seen (radio_id, id);

-- Every packet a MeshCore radio heard, raw, as it logs them: each once, by
-- what names it in all its copies, with what could be read of it.
CREATE TABLE meshcore_packet (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    -- Its type and payload, hashed: what names it whatever way it came.
    key INTEGER NOT NULL,
    -- As the firmware numbers it (4 an advert, 5 a channel's text…).
    payload_type INTEGER NOT NULL,
    -- The live view's kind of it: text, node_info, routing, traceroute,
    -- encrypted or other.
    kind TEXT NOT NULL,
    -- Who sent it and to whom, as far as it says: a node, or NULL.
    from_num INTEGER,
    to_num INTEGER,
    -- The slot of the channel it was opened with, for a channel's text.
    channel INTEGER,
    -- What could be read of it, as JSON.
    info TEXT NOT NULL,
    payload_len INTEGER NOT NULL,
    -- When it was first heard, and last.
    at INTEGER NOT NULL,
    last_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX meshcore_packet_once ON meshcore_packet (radio_id, key);
CREATE INDEX meshcore_packet_radio ON meshcore_packet (radio_id, id);
CREATE INDEX meshcore_packet_at ON meshcore_packet (at);

-- Each copy of it heard, and the way it came.
CREATE TABLE meshcore_packet_copy (
    id INTEGER PRIMARY KEY,
    packet INTEGER NOT NULL REFERENCES meshcore_packet (id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    snr REAL,
    rssi INTEGER,
    -- 0 flooded with region codes, 1 flooded, 2 direct, 3 direct with them.
    route INTEGER NOT NULL,
    -- The region it came in, by its name; ? for one whose key is not known.
    scope TEXT,
    -- The hashes of the repeaters it went through, each hash_size bytes.
    path BLOB NOT NULL,
    hash_size INTEGER NOT NULL
);

CREATE INDEX meshcore_packet_copy_of ON meshcore_packet_copy (packet, id);

-- What the nodes were asked for through each radio, and what they answered:
-- the packet that did, as the radio heard it.
CREATE TABLE request (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    node INTEGER NOT NULL,
    -- node_info, position, device_telemetry, environment_telemetry,
    -- power_telemetry, local_stats, neighbors or regions.
    kind TEXT NOT NULL,
    -- The slot it went out on.
    channel INTEGER NOT NULL,
    -- The id the request went out with, which its answer carries.
    packet_id INTEGER NOT NULL,
    -- pending, answered or failed.
    status TEXT NOT NULL,
    error TEXT,
    at INTEGER NOT NULL,
    answered_at INTEGER,
    answer INTEGER REFERENCES packet_seen (id) ON DELETE SET NULL,
    -- What a node that answers with no packet answered, as JSON.
    reply TEXT
);

CREATE INDEX request_node ON request (radio_id, node, id);
CREATE INDEX request_pending ON request (radio_id, packet_id) WHERE status = 'pending';
