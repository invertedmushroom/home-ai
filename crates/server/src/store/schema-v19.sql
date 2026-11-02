CREATE TABLE model_calls (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT NOT NULL REFERENCES runs(id),
    round INTEGER NOT NULL CHECK(round BETWEEN 1 AND 5),
    resource_group TEXT NOT NULL CHECK(length(resource_group) BETWEEN 1 AND 64),
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    runtime_base_url TEXT NOT NULL,
    runtime_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('dispatching','completed','unresolved','resolved')),
    reason TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    UNIQUE(run_id, round),
    CHECK((status IN ('dispatching','unresolved') AND finished_at_ms IS NULL)
       OR (status IN ('completed','resolved') AND finished_at_ms IS NOT NULL))
);
CREATE UNIQUE INDEX one_owned_local_model_call ON model_calls(resource_group)
    WHERE status IN ('dispatching','unresolved');
CREATE INDEX model_calls_workspace ON model_calls(workspace_id);
PRAGMA user_version=19;
