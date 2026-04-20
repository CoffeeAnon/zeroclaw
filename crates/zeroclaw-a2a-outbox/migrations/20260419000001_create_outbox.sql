CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS outbox (
    id              UUID PRIMARY KEY,
    task_id         TEXT NOT NULL,
    sequence        INTEGER NOT NULL,
    target_url      TEXT NOT NULL,
    auth_token      TEXT,
    payload_json    JSONB NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at    TIMESTAMPTZ,
    last_error      TEXT,
    CONSTRAINT outbox_status_valid CHECK (status IN ('pending', 'delivered', 'deadletter')),
    CONSTRAINT outbox_task_seq_unique UNIQUE (task_id, sequence)
);

CREATE INDEX IF NOT EXISTS outbox_due_idx
    ON outbox (next_attempt_at)
    WHERE status = 'pending';
