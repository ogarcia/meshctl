-- The messages each radio heard or sent. Removing a radio forgets them.
CREATE TABLE message (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    packet_id INTEGER NOT NULL,
    from_num INTEGER NOT NULL,
    -- 4294967295 for a channel message, a node for a direct one.
    to_num INTEGER NOT NULL,
    channel INTEGER NOT NULL,
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
    via_mqtt INTEGER NOT NULL DEFAULT 0
);

-- A packet is heard once: the copies a radio or a tasticsplitter replays on
-- reconnecting are the same message.
CREATE UNIQUE INDEX message_packet ON message (radio_id, from_num, packet_id);

CREATE INDEX message_channel ON message (radio_id, to_num, channel, id);
