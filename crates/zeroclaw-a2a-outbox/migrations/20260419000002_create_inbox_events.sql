CREATE TABLE IF NOT EXISTS inbox_events (
    id              UUID PRIMARY KEY,
    task_id         TEXT NOT NULL,
    sequence        INTEGER NOT NULL,
    payload_json    JSONB NOT NULL,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at    TIMESTAMPTZ,
    CONSTRAINT inbox_events_task_seq_unique UNIQUE (task_id, sequence)
);

CREATE INDEX IF NOT EXISTS inbox_events_unprocessed_idx
    ON inbox_events (received_at)
    WHERE processed_at IS NULL;
