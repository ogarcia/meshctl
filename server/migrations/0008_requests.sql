-- What the nodes were asked for through each radio, and what they answered:
-- the packet that did, as the radio heard it.
CREATE TABLE request (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    node INTEGER NOT NULL,
    -- node_info, position, device_telemetry, environment_telemetry,
    -- power_telemetry, local_stats or neighbors.
    kind TEXT NOT NULL,
    channel INTEGER NOT NULL,
    -- The id the request went out with, which its answer carries.
    packet_id INTEGER NOT NULL,
    -- pending, answered or failed.
    status TEXT NOT NULL,
    error TEXT,
    at INTEGER NOT NULL,
    answered_at INTEGER,
    answer INTEGER REFERENCES packet_seen (id) ON DELETE SET NULL
);

CREATE INDEX request_node ON request (radio_id, node, id);
CREATE INDEX request_pending ON request (radio_id, packet_id) WHERE status = 'pending';
