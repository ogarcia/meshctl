-- How a node came by its position: manual (fixed by hand), internal (its own
-- GPS) or external (a GPS outside it); NULL when it does not say, and for
-- every position kept before this column was.
ALTER TABLE node ADD COLUMN position_source TEXT;
