-- The sender's name as a message gives it, for those that say no more: a
-- MeshCore channel message carries no sender, only the name its client wrote
-- into the text.
ALTER TABLE message ADD COLUMN sender_name TEXT;
