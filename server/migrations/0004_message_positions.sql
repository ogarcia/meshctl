-- A position sent to a conversation, or to the radio, kept among its
-- messages: where, as it went, rounded as its channel shares positions,
-- and how far off that may be. Its text is then empty.
ALTER TABLE message ADD COLUMN latitude REAL;
ALTER TABLE message ADD COLUMN longitude REAL;
ALTER TABLE message ADD COLUMN altitude INTEGER;
ALTER TABLE message ADD COLUMN radius INTEGER;

-- The messages as shown, as before, with where a position is.
DROP VIEW message_view;
CREATE VIEW message_view AS
SELECT
    m.id, m.radio_id, m.packet_id, m.from_num, m.sender_name, m.to_num,
    coalesce(
        (SELECT min(s.slot) FROM channel_slot s WHERE s.channel_id = m.channel_id),
        m.channel
    ) AS channel,
    m.channel_id, m.text, m.at, m.outgoing, m.status, m.error, m.reply_id,
    m.emoji, m.snr, m.rssi, m.hops_away, m.via_mqtt, m.sent_at, m.author_num,
    m.latitude, m.longitude, m.altitude, m.radius,
    coalesce(m.scope, (
        SELECT r.scope FROM message_route r
        WHERE r.radio_id = m.radio_id AND r.packet_id = m.packet_id AND r.scope IS NOT NULL
        ORDER BY r.id LIMIT 1
    )) AS scope,
    (
        SELECT count(*) FROM message_route r
        WHERE r.radio_id = m.radio_id AND r.packet_id = m.packet_id
    ) AS heard,
    (
        SELECT count(*) FROM message_relay h
        WHERE h.radio_id = m.radio_id AND h.packet_id = m.packet_id
    ) AS repeaters
FROM message m;
