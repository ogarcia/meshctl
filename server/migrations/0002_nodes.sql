-- What each radio last knew, so that a radio that is disabled, out of reach,
-- or just starting up still shows it.

-- When the radio was last connected and in sync, in seconds since the epoch.
ALTER TABLE radio ADD COLUMN last_connected INTEGER;

-- The radio itself, as it last described itself.
ALTER TABLE radio ADD COLUMN local_num INTEGER;
ALTER TABLE radio ADD COLUMN local_firmware_version TEXT;
ALTER TABLE radio ADD COLUMN local_hw_model TEXT;
ALTER TABLE radio ADD COLUMN local_role TEXT;
ALTER TABLE radio ADD COLUMN local_region TEXT;
ALTER TABLE radio ADD COLUMN local_modem_preset TEXT;

-- The nodes each radio has heard. Removing a radio forgets them.
CREATE TABLE node (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    num INTEGER NOT NULL,
    long_name TEXT,
    short_name TEXT,
    hw_model TEXT,
    role TEXT,
    latitude REAL,
    longitude REAL,
    altitude INTEGER,
    last_heard INTEGER,
    snr REAL,
    rssi INTEGER,
    hops_away INTEGER,
    via_mqtt INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    battery_level INTEGER,
    voltage REAL,
    PRIMARY KEY (radio_id, num)
);
