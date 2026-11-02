CREATE TABLE revisions_v7 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    edit_id TEXT UNIQUE REFERENCES proposals(edit_id),
    number INTEGER NOT NULL,
    html TEXT NOT NULL,
    files TEXT NOT NULL,
    UNIQUE(workspace_id, number),
    CHECK((run_id IS NOT NULL)+(execution_id IS NOT NULL)+(edit_id IS NOT NULL)=1)
);
INSERT INTO revisions_v7(rowid,id,workspace_id,run_id,execution_id,number,html,files)
    SELECT rowid,id,workspace_id,run_id,execution_id,number,html,files FROM revisions;
DROP TABLE revisions;
ALTER TABLE revisions_v7 RENAME TO revisions;

CREATE TABLE proposals_v7 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    edit_id TEXT UNIQUE,
    base_revision_id TEXT REFERENCES revisions(id),
    status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected')),
    changes TEXT NOT NULL,
    applied_revision_id TEXT UNIQUE REFERENCES revisions(id),
    rejection_reason TEXT CHECK(rejection_reason IS NULL OR
        (status='rejected' AND length(CAST(rejection_reason AS BLOB)) BETWEEN 1 AND 4096)),
    CHECK((run_id IS NOT NULL)+(execution_id IS NOT NULL)+(edit_id IS NOT NULL)=1),
    CHECK((status='applied' AND applied_revision_id IS NOT NULL) OR (status!='applied' AND applied_revision_id IS NULL))
);
INSERT INTO proposals_v7(rowid,id,workspace_id,run_id,execution_id,base_revision_id,status,changes,applied_revision_id,rejection_reason)
    SELECT rowid,id,workspace_id,run_id,execution_id,base_revision_id,status,changes,applied_revision_id,rejection_reason FROM proposals;
DROP TABLE proposals;
ALTER TABLE proposals_v7 RENAME TO proposals;
PRAGMA user_version=7;
