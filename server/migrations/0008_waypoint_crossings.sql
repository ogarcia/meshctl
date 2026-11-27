-- Nodes gone in or out of the fences meshctl watches: the latest of each
-- waypoint's, gone with it.
CREATE TABLE waypoint_crossing (
    id INTEGER PRIMARY KEY,
    radio_id INTEGER NOT NULL,
    waypoint_id INTEGER NOT NULL,
    node INTEGER NOT NULL,
    -- 1 gone in, 0 gone out.
    entered INTEGER NOT NULL,
    at INTEGER NOT NULL,
    FOREIGN KEY (radio_id, waypoint_id) REFERENCES waypoint (radio_id, id) ON DELETE CASCADE
);

CREATE INDEX waypoint_crossing_waypoint ON waypoint_crossing (radio_id, waypoint_id, at);
