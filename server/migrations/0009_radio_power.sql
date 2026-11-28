-- How a radio is powered, to show its battery: auto, as its battery says;
-- battery, always shown; plugged, never.
ALTER TABLE radio ADD COLUMN power TEXT NOT NULL DEFAULT 'auto';
