use super::ACTIVE_RUN_PREDICATE;
use anyhow::{Result, bail};
use rusqlite::Connection;
use uuid::Uuid;

pub(super) fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 0..=23) {
        bail!("database schema version {version} is newer than this application supports");
    }
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    if version == 0 {
        connection.execute_batch(&format!(
            "
            CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed','cancelled','interrupted')),
                error TEXT, provider TEXT NOT NULL, model TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS one_active_run ON runs(workspace_id)
                WHERE {ACTIVE_RUN_PREDICATE};
            CREATE TABLE IF NOT EXISTS messages (
                seq INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                run_id TEXT NOT NULL REFERENCES runs(id), role TEXT NOT NULL, content TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS revisions (
                id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                run_id TEXT NOT NULL UNIQUE REFERENCES runs(id), number INTEGER NOT NULL, html TEXT NOT NULL,
                UNIQUE(workspace_id, number)
            );
        "
        ))?;
    }
    if version < 23 {
        // SQLite's documented table-rebuild procedure. This connection is
        // private until all pending migrations and FK checks commit.
        connection.pragma_update(None, "foreign_keys", false)?;
        let tx = connection.transaction()?;
        if version < 2 {
            tx.execute_batch(
                "
                ALTER TABLE runs ADD COLUMN output TEXT NOT NULL DEFAULT '';
                ALTER TABLE runs ADD COLUMN stats TEXT NOT NULL DEFAULT '{}';
                ALTER TABLE runs ADD COLUMN created_at_ms INTEGER;
                ALTER TABLE runs ADD COLUMN started_at_ms INTEGER;
                ALTER TABLE runs ADD COLUMN finished_at_ms INTEGER;
                PRAGMA user_version=2;
            ",
            )?;
        }
        if version < 3 {
            tx.execute_batch(
                "
                ALTER TABLE revisions ADD COLUMN files TEXT NOT NULL DEFAULT '[]';
                UPDATE revisions SET files=json_array(json_object('path','index.html','content',html));
                ALTER TABLE runs ADD COLUMN mode TEXT NOT NULL DEFAULT 'chat' CHECK(mode IN ('chat','app','files'));
                UPDATE runs SET mode='app' WHERE id IN (SELECT run_id FROM revisions);
                ALTER TABLE runs ADD COLUMN base_revision_id TEXT REFERENCES revisions(id);
                CREATE TABLE proposals (
                    id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id), base_revision_id TEXT REFERENCES revisions(id),
                    status TEXT NOT NULL CHECK(status IN ('pending','applied','rejected')),
                    changes TEXT NOT NULL, applied_revision_id TEXT UNIQUE REFERENCES revisions(id),
                    CHECK((status='applied' AND applied_revision_id IS NOT NULL) OR (status!='applied' AND applied_revision_id IS NULL))
                );
                PRAGMA user_version=3;
            ",
            )?;
        }
        if version < 4 {
            tx.execute_batch(
                "
                CREATE TABLE worker_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1),owner TEXT NOT NULL,path TEXT,endpoint TEXT,daemon TEXT);
                CREATE TABLE executions(
                    id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                    revision_id TEXT NOT NULL REFERENCES revisions(id),entry TEXT NOT NULL,
                    action TEXT NOT NULL CHECK(action IN ('check','run')),image_id TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('queued','running','cancelling','succeeded','failed','cancelled','timed_out','interrupted')),
                    stdout TEXT NOT NULL DEFAULT '',stderr TEXT NOT NULL DEFAULT '',
                    exit_code INTEGER,signal TEXT,truncated INTEGER NOT NULL DEFAULT 0,error TEXT,
                    created_at_ms INTEGER NOT NULL,finished_at_ms INTEGER
                );
                CREATE UNIQUE INDEX one_active_execution ON executions((1)) WHERE status IN ('queued','running','cancelling');
                PRAGMA user_version=4;
            ",
            )?;
            tx.execute(
                "INSERT INTO worker_state(singleton,owner) VALUES (1,?1)",
                [Uuid::new_v4().to_string()],
            )?;
        }
        if version < 5 {
            tx.execute_batch(include_str!("schema-v5.sql"))?;
        }
        if version < 6 {
            tx.execute_batch(include_str!("schema-v6.sql"))?;
        }
        if version < 7 {
            tx.execute_batch(include_str!("schema-v7.sql"))?;
        }
        if version < 8 {
            tx.execute_batch(include_str!("schema-v8.sql"))?;
        }
        if version < 9 {
            tx.execute_batch(
                "ALTER TABLE runs ADD COLUMN execution_result TEXT; PRAGMA user_version=9;",
            )?;
        }
        if version < 10 {
            tx.execute_batch(
                "ALTER TABLE runs ADD COLUMN search_config TEXT;
                ALTER TABLE runs ADD COLUMN search_activity TEXT NOT NULL DEFAULT '[]';
                ALTER TABLE runs ADD COLUMN model_rounds INTEGER NOT NULL DEFAULT 0;
                PRAGMA user_version=10;",
            )?;
        }
        if version < 11 {
            tx.execute_batch(include_str!("schema-v11.sql"))?;
        }
        if version < 12 {
            tx.execute_batch(include_str!("schema-v12.sql"))?;
        }
        if version < 13 {
            tx.execute_batch(include_str!("schema-v13.sql"))?;
        }
        if version < 14 {
            tx.execute_batch(include_str!("schema-v14.sql"))?;
        }
        if version < 15 {
            tx.execute_batch(include_str!("schema-v15.sql"))?;
        }
        if version < 16 {
            tx.execute_batch(include_str!("schema-v16.sql"))?;
        }
        if version < 17 {
            tx.execute_batch(
                "ALTER TABLE runs ADD COLUMN image_input TEXT; PRAGMA user_version=17;",
            )?;
        }
        if version < 18 {
            tx.execute_batch(include_str!("schema-v18.sql"))?;
        }
        if version < 19 {
            tx.execute_batch(include_str!("schema-v19.sql"))?;
        }
        // Planned images may queue together. Dispatch remains serialized by
        // shared group slots and the transactional group/runtime ownership check.
        if version < 20 {
            tx.execute_batch("DROP INDEX IF EXISTS one_active_image_job; PRAGMA user_version=20;")?;
        }
        if version < 21 {
            tx.execute_batch("ALTER TABLE runs ADD COLUMN workspace_tools INTEGER NOT NULL DEFAULT 0 CHECK(workspace_tools IN (0,1)); PRAGMA user_version=21;")?;
        }
        if version < 22 {
            tx.execute_batch(
                "ALTER TABLE runs ADD COLUMN model_route TEXT; PRAGMA user_version=22;",
            )?;
        }
        tx.execute_batch("ALTER TABLE runs ADD COLUMN image_attachments TEXT NOT NULL DEFAULT '[]'; PRAGMA user_version=23;")?;
        let invalid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )?;
        if invalid {
            bail!("schema migration failed foreign-key verification");
        }
        tx.commit()?;
        connection.pragma_update(None, "foreign_keys", true)?;
    }
    Ok(())
}
