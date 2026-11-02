PRAGMA foreign_keys=ON;
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE runs (
    id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed','cancelled','interrupted')),
    error TEXT, provider TEXT NOT NULL, model TEXT NOT NULL
);
CREATE UNIQUE INDEX one_active_run ON runs(workspace_id) WHERE status IN ('queued','running');
CREATE TABLE messages (
    seq INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT NOT NULL REFERENCES runs(id), role TEXT NOT NULL, content TEXT NOT NULL
);
CREATE TABLE revisions (
    id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id), number INTEGER NOT NULL, html TEXT NOT NULL,
    UNIQUE(workspace_id, number)
);
PRAGMA user_version=1;
INSERT INTO workspaces(id,title) VALUES ('workspace','Existing work');
INSERT INTO runs(id,workspace_id,status,provider,model) VALUES
    ('complete','workspace','succeeded','compatible','original-model'),
    ('pending','workspace','queued','demo','demo-model');
INSERT INTO messages(workspace_id,run_id,role,content) VALUES
    ('workspace','complete','user','Original prompt'),
    ('workspace','complete','assistant','Original reply');
INSERT INTO revisions(id,workspace_id,run_id,number,html) VALUES
    ('original','workspace','complete',1,'<h1>Original</h1>');
