-- Where messages are notified of: on a radio, one channel, wherever it is,
-- maybe only when it mentions the radio; one node's direct conversation;
-- or, neither, every direct conversation. Whether notifications are on, and
-- whether these places were ever set, go among the preferences.
CREATE TABLE notification_place (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    channel_id INTEGER REFERENCES channel (id) ON DELETE CASCADE,
    node INTEGER,
    mentions INTEGER NOT NULL DEFAULT 0,
    CHECK (channel_id IS NULL OR node IS NULL)
);

CREATE UNIQUE INDEX notification_place_once ON notification_place
    (radio_id, ifnull(channel_id, -1), ifnull(node, -1));
CREATE INDEX notification_place_radio ON notification_place (radio_id);
