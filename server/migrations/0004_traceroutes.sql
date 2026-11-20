-- The traceroutes each radio sent. Removing a radio forgets them.
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
