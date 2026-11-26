-- How long ago a radio's nodes have to have been heard to be on its map, in
-- seconds; none for every node.
ALTER TABLE radio ADD COLUMN map_max_age INTEGER;
