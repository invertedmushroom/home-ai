CREATE TABLE guests (
    id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL
);
ALTER TABLE workspaces ADD COLUMN guest_id TEXT REFERENCES guests(id);
CREATE INDEX workspaces_guest ON workspaces(guest_id);
PRAGMA user_version=16;
