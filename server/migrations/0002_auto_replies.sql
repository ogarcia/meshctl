-- Automatic replies: what a message must say for meshctl to answer it on
-- its own, and how, in the order they are tried in.
CREATE TABLE auto_reply (
    id INTEGER PRIMARY KEY,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    -- What a message says to be answered, as a JSON array of text.
    triggers TEXT NOT NULL,
    -- 'whole' or 'word'.
    matching TEXT NOT NULL,
    react INTEGER NOT NULL,
    reply TEXT,
    direct_reply TEXT
);

-- Where each answers: on a radio, one channel, wherever it is; one node's
-- direct conversation; or, neither, every direct conversation.
CREATE TABLE auto_reply_place (
    auto_reply_id INTEGER NOT NULL REFERENCES auto_reply (id) ON DELETE CASCADE,
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    channel_id INTEGER REFERENCES channel (id) ON DELETE CASCADE,
    node INTEGER,
    CHECK (channel_id IS NULL OR node IS NULL)
);

CREATE UNIQUE INDEX auto_reply_place_once ON auto_reply_place
    (auto_reply_id, radio_id, ifnull(channel_id, -1), ifnull(node, -1));
CREATE INDEX auto_reply_place_radio ON auto_reply_place (radio_id);
