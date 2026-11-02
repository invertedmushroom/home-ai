PRAGMA foreign_keys=ON;
CREATE TABLE workspaces(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE runs(
 id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),status TEXT NOT NULL,error TEXT,provider TEXT NOT NULL,model TEXT NOT NULL,
 output TEXT NOT NULL DEFAULT '',stats TEXT NOT NULL DEFAULT '{}',created_at_ms INTEGER,started_at_ms INTEGER,finished_at_ms INTEGER,
 mode TEXT NOT NULL DEFAULT 'chat',base_revision_id TEXT REFERENCES revisions(id)
);
CREATE UNIQUE INDEX one_active_run ON runs(workspace_id) WHERE status IN ('queued','running');
CREATE TABLE messages(seq INTEGER PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),run_id TEXT NOT NULL REFERENCES runs(id),role TEXT NOT NULL,content TEXT NOT NULL);
CREATE TABLE revisions(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),number INTEGER NOT NULL,html TEXT NOT NULL,files TEXT NOT NULL DEFAULT '[]',UNIQUE(workspace_id,number));
CREATE TABLE proposals(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),base_revision_id TEXT REFERENCES revisions(id),status TEXT NOT NULL,changes TEXT NOT NULL,applied_revision_id TEXT UNIQUE REFERENCES revisions(id));
CREATE TABLE worker_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1),owner TEXT NOT NULL,path TEXT,endpoint TEXT,daemon TEXT);
CREATE TABLE executions(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),revision_id TEXT NOT NULL REFERENCES revisions(id),entry TEXT NOT NULL,action TEXT NOT NULL,image_id TEXT NOT NULL,status TEXT NOT NULL,stdout TEXT NOT NULL DEFAULT '',stderr TEXT NOT NULL DEFAULT '',exit_code INTEGER,signal TEXT,truncated INTEGER NOT NULL DEFAULT 0,error TEXT,created_at_ms INTEGER NOT NULL,finished_at_ms INTEGER);
CREATE UNIQUE INDEX one_active_execution ON executions((1)) WHERE status IN ('queued','running','cancelling');
PRAGMA user_version=4;
INSERT INTO workspaces(id,title) VALUES ('workspace','Existing project');
INSERT INTO runs(id,workspace_id,status,provider,model,mode) VALUES ('first','workspace','succeeded','demo','demo-model','app');
INSERT INTO revisions(id,workspace_id,run_id,number,html,files) VALUES ('original','workspace','first',1,'<h1>Original</h1>','[{"path":"index.html","content":"<h1>Original</h1>"},{"path":"keep.txt","content":"Exact\r\n界🙂"}]');
INSERT INTO runs(id,workspace_id,status,provider,model,mode,base_revision_id,stats,created_at_ms,started_at_ms,finished_at_ms) VALUES
 ('change','workspace','succeeded','openai','old-model','files','original','{"input_tokens":17,"reported_model":"reported-v4"}',100,120,200);
INSERT INTO revisions(id,workspace_id,run_id,number,html,files) VALUES ('applied','workspace','change',2,'<h1>Changed</h1>','[{"path":"index.html","content":"<h1>Changed</h1>"},{"path":"keep.txt","content":"Exact\r\n界🙂"}]');
INSERT INTO proposals VALUES ('applied-proposal','workspace','change','original','applied','[{"op":"write","path":"index.html","content":"<h1>Changed</h1>"}]','applied');
INSERT INTO runs(id,workspace_id,status,provider,model,mode,base_revision_id) VALUES
 ('pending-run','workspace','succeeded','demo','demo-model','files','applied'),
 ('rejected-run','workspace','succeeded','demo','demo-model','files','applied'),
 ('unfinished','workspace','queued','demo','demo-model','chat',NULL);
INSERT INTO proposals VALUES
 ('pending-proposal','workspace','pending-run','applied','pending','[{"op":"write","path":"future.txt","content":"later"}]',NULL),
 ('rejected-proposal','workspace','rejected-run','applied','rejected','[{"op":"write","path":"rejected.txt","content":"no"}]',NULL);
INSERT INTO messages(workspace_id,run_id,role,content) VALUES ('workspace','change','user','Original prompt'),('workspace','change','assistant','Original reply');
INSERT INTO worker_state VALUES(1,'45a7273f-656e-43ca-8e67-d228d9330568','original-path','unix:///var/run/docker.sock','original-daemon');
INSERT INTO executions(id,workspace_id,revision_id,entry,action,image_id,status,stdout,exit_code,created_at_ms,finished_at_ms) VALUES ('done','workspace','applied','index.html','check','old-image','succeeded','old stdout',0,300,400);
INSERT INTO executions(id,workspace_id,revision_id,entry,action,image_id,status,created_at_ms) VALUES ('active','workspace','applied','index.html','check','old-image','queued',500);
