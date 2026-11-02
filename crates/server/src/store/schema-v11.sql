ALTER TABLE runs ADD COLUMN execution_revision_id TEXT REFERENCES revisions(id);
CREATE TABLE run_executions (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    call_id TEXT NOT NULL,
    execution_id TEXT NOT NULL UNIQUE,
    entry TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('check','run')),
    result TEXT
);
PRAGMA user_version=11;
