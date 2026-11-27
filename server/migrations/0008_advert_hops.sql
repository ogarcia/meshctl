-- On MeshCore, how many hops a node's latest advert came through, the fewest
-- of its copies: how far it is heard from, not a way to it. None until one
-- is heard; on Meshtastic, none.
ALTER TABLE node ADD COLUMN advert_hops INTEGER;
