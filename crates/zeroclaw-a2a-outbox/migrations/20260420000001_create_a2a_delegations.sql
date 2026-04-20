CREATE TABLE IF NOT EXISTS a2a_delegations (
    task_id    TEXT PRIMARY KEY,
    session_id TEXT,
    prompt     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS a2a_delegations_session_idx ON a2a_delegations (session_id);
