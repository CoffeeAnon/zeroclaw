CREATE TABLE IF NOT EXISTS push_notification_configs (
    task_id    TEXT NOT NULL,
    config_id  TEXT NOT NULL DEFAULT '',
    url        TEXT NOT NULL,
    token      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (task_id, config_id)
);
