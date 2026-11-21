-- Each radio's channels as last read, by slot: a fingerprint of each one's
-- name and key, never the key, to tell a channel that moved, and whose
-- history goes with it, from one edited where it is.
CREATE TABLE channel_slot (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    slot INTEGER NOT NULL,
    fingerprint BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (radio_id, slot)
);

-- Channels taken off a radio whose history was kept: to give it back if the
-- channel comes back, and to find it to clear it.
CREATE TABLE former_channel (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    fingerprint BLOB NOT NULL,
    name TEXT NOT NULL,
    removed_at INTEGER NOT NULL,
    PRIMARY KEY (radio_id, fingerprint)
);

-- The channel a message was on, once that channel is gone: its fingerprint.
-- Such a message is in no conversation until the channel comes back.
ALTER TABLE message ADD COLUMN former BLOB;
CREATE INDEX message_former ON message (radio_id, former) WHERE former IS NOT NULL;
