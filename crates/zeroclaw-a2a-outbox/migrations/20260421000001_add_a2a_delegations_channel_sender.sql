-- Capture the originating channel + sender for each delegation so the
-- inbox drain can deliver Sam's reply back to the conversation that
-- triggered the ask_walter call. Nullable because non-channel callers
-- (tests, CLI) won't have one.
ALTER TABLE a2a_delegations ADD COLUMN IF NOT EXISTS channel TEXT;
ALTER TABLE a2a_delegations ADD COLUMN IF NOT EXISTS sender TEXT;
