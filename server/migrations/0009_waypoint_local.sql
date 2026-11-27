-- Whether a waypoint is meshctl's alone, never sent: one to see on its map,
-- as the iOS app's "local only".
ALTER TABLE waypoint ADD COLUMN local INTEGER NOT NULL DEFAULT 0;
