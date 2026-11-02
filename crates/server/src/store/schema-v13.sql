ALTER TABLE runs ADD COLUMN image_config TEXT;

CREATE TABLE image_jobs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
    call_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    seed INTEGER NOT NULL,
    settings TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('planned','submitting','running','cancelling','unresolved','succeeded','failed','cancelled','interrupted')),
    remote_job_id TEXT,
    agent_context TEXT,
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER
);
CREATE UNIQUE INDEX one_active_image_job ON image_jobs((1))
    WHERE status IN ('planned','submitting','running','cancelling','unresolved');

CREATE TABLE image_assets (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    image_job_id TEXT NOT NULL UNIQUE REFERENCES image_jobs(id),
    media_type TEXT NOT NULL CHECK(media_type IN ('image/png','image/jpeg')),
    bytes INTEGER NOT NULL CHECK(bytes BETWEEN 1 AND 8388608),
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 2048),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 2048),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    content BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    CHECK(length(content)=bytes)
);
PRAGMA user_version=13;
