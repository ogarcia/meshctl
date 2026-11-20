-- How busy a node hears the channel, as a percentage of the time, from its
-- device telemetry; NULL until it sends some.
ALTER TABLE node ADD COLUMN channel_utilization REAL;
