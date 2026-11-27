-- The Meshtastic waypoints each radio heard or shared, by their own ids: as
-- they last were, each replacing the one before, and gone once deleted or
-- expired. And meshctl's own, never sent, on any radio.
CREATE TABLE waypoint (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    id INTEGER NOT NULL,
    latitude REAL NOT NULL,
    longitude REAL NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    -- Its emoji, as a Unicode code point; 0 for none.
    icon INTEGER NOT NULL,
    -- When it expires, in seconds since the epoch; 0, never.
    expire INTEGER NOT NULL,
    -- The node it is locked to; 0 for anyone.
    locked_to INTEGER NOT NULL,
    -- Its fence: a circle's radius in metres (0, none), a box (NULL, none),
    -- and what its author asks to be told of.
    fence_radius INTEGER NOT NULL,
    fence_south REAL,
    fence_west REAL,
    fence_north REAL,
    fence_east REAL,
    notify_on_enter INTEGER NOT NULL,
    notify_on_exit INTEGER NOT NULL,
    notify_favorites_only INTEGER NOT NULL,
    -- Whether meshctl tells of nodes going in and out of its fence: of its
    -- own, as their author asks; of another's, once asked to.
    watched INTEGER NOT NULL DEFAULT 0,
    -- Whether it is meshctl's alone, never sent: one to see on its map, as
    -- the iOS app's "local only".
    local INTEGER NOT NULL DEFAULT 0,
    -- How many hops the radio sends one of its own with, and deletes it
    -- with; none for as many as the radio's settings say.
    hops INTEGER,
    -- The slot it was last heard or sent on.
    channel INTEGER NOT NULL,
    created_by INTEGER NOT NULL,
    updated_by INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (radio_id, id)
);

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

-- Whether a radio's map shows its waypoints.
ALTER TABLE radio ADD COLUMN map_waypoints INTEGER NOT NULL DEFAULT 1;
