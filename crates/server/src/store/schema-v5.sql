ALTER TABLE executions ADD COLUMN output_files TEXT NOT NULL DEFAULT '[]';

CREATE TABLE revisions_v5 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    number INTEGER NOT NULL,
    html TEXT NOT NULL,
    files TEXT NOT NULL,
    UNIQUE(workspace_id, number),
    CHECK((run_id IS NOT NULL) != (execution_id IS NOT NULL))
);
INSERT INTO revisions_v5(id,workspace_id,run_id,number,html,files)
    SELECT id,workspace_id,run_id,number,html,files FROM revisions;
DROP TABLE revisions;
ALTER TABLE revisions_v5 RENAME TO revisions;

CREATE TABLE proposals_v5 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    base_revision_id TEXT REFERENCES revisions(id),
    status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected')),
    changes TEXT NOT NULL,
    applied_revision_id TEXT UNIQUE REFERENCES revisions(id),
    CHECK((run_id IS NOT NULL) != (execution_id IS NOT NULL)),
    CHECK((status='applied' AND applied_revision_id IS NOT NULL) OR (status!='applied' AND applied_revision_id IS NULL))
);
INSERT INTO proposals_v5(rowid,id,workspace_id,run_id,base_revision_id,status,changes,applied_revision_id)
    SELECT rowid,id,workspace_id,run_id,base_revision_id,status,changes,applied_revision_id FROM proposals;
DROP TABLE proposals;
ALTER TABLE proposals_v5 RENAME TO proposals;
PRAGMA user_version=5;
