-- How far, in metres, a node's position may be from where it is, when it
-- shares it rounded, as most do; NULL when it is exact or does not say, and
-- for every position kept before this column was.
ALTER TABLE node ADD COLUMN position_radius INTEGER;
