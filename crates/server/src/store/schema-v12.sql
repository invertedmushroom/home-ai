CREATE TABLE executions_v12 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    revision_id TEXT REFERENCES revisions(id),
    entry TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('check','run')),
    code TEXT,
    input_assets TEXT NOT NULL DEFAULT '[]',
    image_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','running','cancelling','succeeded','failed','cancelled','timed_out','interrupted')),
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    exit_code INTEGER,
    signal TEXT,
    truncated INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    output_files TEXT NOT NULL DEFAULT '[]',
    CHECK(
        (revision_id IS NOT NULL AND code IS NULL)
        OR (revision_id IS NULL AND code IS NOT NULL AND action = 'run')
    )
);
INSERT INTO executions_v12(rowid,id,workspace_id,revision_id,entry,action,code,input_assets,image_id,status,stdout,stderr,exit_code,signal,truncated,error,created_at_ms,finished_at_ms,output_files)
    SELECT rowid,id,workspace_id,revision_id,entry,action,NULL,'[]',image_id,status,stdout,stderr,exit_code,signal,truncated,error,created_at_ms,finished_at_ms,output_files FROM executions;
DROP TABLE executions;
ALTER TABLE executions_v12 RENAME TO executions;
CREATE UNIQUE INDEX one_active_execution ON executions((1)) WHERE status IN ('queued','running','cancelling');

CREATE TABLE assets (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    content TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('upload', 'execution')),
    execution_id TEXT REFERENCES executions(id),
    created_at_ms INTEGER NOT NULL,
    CHECK((origin = 'execution' AND execution_id IS NOT NULL) OR (origin = 'upload' AND execution_id IS NULL))
);

ALTER TABLE runs ADD COLUMN node_scope TEXT NOT NULL DEFAULT 'off';
UPDATE runs SET node_scope = 'revision' WHERE execution_revision_id IS NOT NULL;
ALTER TABLE runs ADD COLUMN input_assets TEXT NOT NULL DEFAULT '[]';

CREATE TABLE run_executions_v12 (
    execution_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK(seq >= 1),
    call_id TEXT NOT NULL,
    entry TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('check','run')),
    code TEXT,
    input_assets TEXT NOT NULL DEFAULT '[]',
    result TEXT,
    UNIQUE(run_id, call_id),
    UNIQUE(run_id, seq)
);
INSERT INTO run_executions_v12(execution_id,run_id,seq,call_id,entry,action,code,input_assets,result)
    SELECT execution_id,run_id,1,call_id,entry,action,NULL,'[]',result FROM run_executions;
DROP TABLE run_executions;
ALTER TABLE run_executions_v12 RENAME TO run_executions;

CREATE TABLE revisions_v12 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    edit_id TEXT UNIQUE REFERENCES proposals(edit_id),
    asset_ids TEXT,
    number INTEGER NOT NULL,
    html TEXT NOT NULL,
    files TEXT NOT NULL,
    UNIQUE(workspace_id, number),
    CHECK((run_id IS NOT NULL)+(execution_id IS NOT NULL)+(edit_id IS NOT NULL)+(asset_ids IS NOT NULL)=1)
);
INSERT INTO revisions_v12(rowid,id,workspace_id,run_id,execution_id,edit_id,asset_ids,number,html,files)
    SELECT rowid,id,workspace_id,run_id,execution_id,edit_id,NULL,number,html,files FROM revisions;
DROP TABLE revisions;
ALTER TABLE revisions_v12 RENAME TO revisions;

CREATE TABLE proposals_v12 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT UNIQUE REFERENCES runs(id),
    execution_id TEXT UNIQUE REFERENCES executions(id),
    edit_id TEXT UNIQUE,
    asset_ids TEXT,
    base_revision_id TEXT REFERENCES revisions(id),
    status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected')),
    changes TEXT NOT NULL,
    applied_revision_id TEXT UNIQUE REFERENCES revisions(id),
    rejection_reason TEXT CHECK(rejection_reason IS NULL OR
        (status='rejected' AND length(CAST(rejection_reason AS BLOB)) BETWEEN 1 AND 4096)),
    CHECK((run_id IS NOT NULL)+(execution_id IS NOT NULL)+(edit_id IS NOT NULL)+(asset_ids IS NOT NULL)=1),
    CHECK((status='applied' AND applied_revision_id IS NOT NULL) OR (status!='applied' AND applied_revision_id IS NULL))
);
INSERT INTO proposals_v12(rowid,id,workspace_id,run_id,execution_id,edit_id,asset_ids,base_revision_id,status,changes,applied_revision_id,rejection_reason)
    SELECT rowid,id,workspace_id,run_id,execution_id,edit_id,NULL,base_revision_id,status,changes,applied_revision_id,rejection_reason FROM proposals;
DROP TABLE proposals;
ALTER TABLE proposals_v12 RENAME TO proposals;
PRAGMA user_version=12;
