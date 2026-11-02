ALTER TABLE runs ADD COLUMN import_enabled INTEGER NOT NULL DEFAULT 0 CHECK(import_enabled IN (0,1));
CREATE TABLE assets_v18 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    content TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('upload','execution','import')),
    execution_id TEXT REFERENCES executions(id),
    created_at_ms INTEGER NOT NULL,
    source TEXT,
    CHECK((origin='execution') = (execution_id IS NOT NULL)),
    CHECK((origin='import') = (source IS NOT NULL))
);
INSERT INTO assets_v18(rowid,id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms)
    SELECT rowid,id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms FROM assets;
DROP TABLE assets;
ALTER TABLE assets_v18 RENAME TO assets;
CREATE INDEX imported_assets_by_run ON assets(json_extract(source,'$.run_id')) WHERE origin='import';
PRAGMA user_version=18;
