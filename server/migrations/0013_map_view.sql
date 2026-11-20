-- Where the interface's map was left for each radio, as the JSON of the API's
-- MapView: NULL until it is first moved.
ALTER TABLE radio ADD COLUMN map_view TEXT;
