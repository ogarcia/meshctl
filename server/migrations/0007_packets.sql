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
    -- When it was first heard, by any radio.
    at INTEGER NOT NULL
);

CREATE UNIQUE INDEX packet_identity ON packet (from_num, packet_id);
CREATE INDEX packet_to ON packet (to_num, at);
CREATE INDEX packet_request ON packet (request_id) WHERE request_id IS NOT NULL;

-- Each radio's hearing of a packet. A radio, or a tasticsplitter, replays
-- packets on reconnecting: the same radio hears a packet once.
CREATE TABLE packet_seen (
    id INTEGER PRIMARY KEY,
    packet INTEGER NOT NULL REFERENCES packet (id) ON DELETE CASCADE,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    -- The channel's slot on this radio, which another radio may have elsewhere.
    channel INTEGER NOT NULL,
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
