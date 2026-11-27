-- How many hops the radio sends one of its own waypoints with, and deletes
-- it with; none for as many as the radio's settings say.
ALTER TABLE waypoint ADD COLUMN hops INTEGER;
