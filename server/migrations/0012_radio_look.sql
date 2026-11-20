-- How each radio shows on the interface's rail: whether it does, in which
-- place, and how its button looks, as the JSON of the API's Badge (NULL for
-- the rail's colour and the radio's initials).
ALTER TABLE radio ADD COLUMN shown INTEGER NOT NULL DEFAULT 1;
ALTER TABLE radio ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE radio ADD COLUMN badge TEXT;

-- The radios there are keep the order they had: the one they were added in.
UPDATE radio SET position = id;
