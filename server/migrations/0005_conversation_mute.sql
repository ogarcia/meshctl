-- The conversations of a radio silenced: kept out of what is unread and of
-- the notifications, and away while their section is folded. A channel's,
-- wherever the channel is now; or a node's.
CREATE TABLE conversation_mute (
    radio_id INTEGER NOT NULL REFERENCES radio (id) ON DELETE CASCADE,
    channel_id INTEGER REFERENCES channel (id) ON DELETE CASCADE,
    node INTEGER,
    CHECK ((channel_id IS NULL) != (node IS NULL))
);

CREATE UNIQUE INDEX conversation_mute_channel ON conversation_mute (channel_id)
    WHERE channel_id IS NOT NULL;
CREATE UNIQUE INDEX conversation_mute_node ON conversation_mute (radio_id, node)
    WHERE node IS NOT NULL;
