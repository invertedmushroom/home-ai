use super::{ArtifactConflict, ExecutionError, Store, Workspace, workspace_busy};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

fn remap_value(value: &mut Value, id_map: &HashMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(mapped) = id_map.get(text.as_str()) {
                *text = mapped.clone();
            }
        }
        Value::Array(items) => {
            for item in items {
                remap_value(item, id_map);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                remap_value(item, id_map);
            }
        }
        _ => {}
    }
}

fn remap_json(raw: &str, id_map: &HashMap<String, String>) -> Result<String> {
    let mut value: Value = serde_json::from_str(raw)?;
    remap_value(&mut value, id_map);
    Ok(serde_json::to_string(&value)?)
}

fn remap_opt_json(raw: Option<String>, id_map: &HashMap<String, String>) -> Result<Option<String>> {
    raw.map(|text| remap_json(&text, id_map)).transpose()
}

fn map_id(id: &str, id_map: &HashMap<String, String>) -> Result<String> {
    id_map
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing cloned identifier mapping for {id}"))
}

fn map_opt_id(id: Option<String>, id_map: &HashMap<String, String>) -> Result<Option<String>> {
    id.map(|value| map_id(&value, id_map)).transpose()
}

fn register_ids(
    c: &Connection,
    sql: &str,
    workspace_id: &str,
    id_map: &mut HashMap<String, String>,
) -> Result<()> {
    let mut stmt = c.prepare(sql)?;
    let rows = stmt.query_map([workspace_id], |row| row.get::<_, Option<String>>(0))?;
    for row in rows {
        if let Some(id) = row? {
            id_map
                .entry(id)
                .or_insert_with(|| Uuid::new_v4().to_string());
        }
    }
    Ok(())
}

impl Store {
    pub async fn delete_turn(&self, workspace_id: String, run_id: String) -> Result<Option<()>> {
        if Uuid::parse_str(&run_id).is_err() {
            return Ok(None);
        }
        self.call(move |c| {
            let tx = c.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
                [&workspace_id],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            if workspace_busy(&tx, &workspace_id)? {
                return Err(ArtifactConflict::WorkspaceBusy.into());
            }
            let run_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1 AND workspace_id=?2)",
                params![run_id, workspace_id],
                |r| r.get(0),
            )?;
            if !run_exists {
                return Ok(None);
            }
            tx.execute(
                "DELETE FROM messages WHERE workspace_id=?1 AND run_id=?2",
                params![workspace_id, run_id],
            )?;
            tx.commit()?;
            Ok(Some(()))
        })
        .await
    }

    pub async fn branch_workspace(
        &self,
        source_id: String,
        title: Option<String>,
        up_to_run_id: Option<String>,
    ) -> Result<Option<Workspace>> {
        if let Some(ref run_id) = up_to_run_id
            && Uuid::parse_str(run_id).is_err()
        {
            return Err(ExecutionError(
                axum::http::StatusCode::BAD_REQUEST,
                "Turn ID must be a valid UUID.",
            )
            .into());
        }
        self.call(move |c| {
            let tx = c.transaction()?;
            let source: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT title, guest_id FROM workspaces WHERE id=?1",
                    [&source_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((source_title, guest_id)) = source else {
                return Ok(None);
            };
            if workspace_busy(&tx, &source_id)? {
                return Err(ArtifactConflict::WorkspaceBusy.into());
            }
            let max_seq: Option<i64> = match up_to_run_id {
                Some(target_run_id) => {
                    let seq: Option<i64> = tx.query_row(
                        "SELECT MAX(seq) FROM messages WHERE workspace_id=?1 AND run_id=?2",
                        params![source_id, target_run_id],
                        |r| r.get(0),
                    )?;
                    let Some(seq) = seq else {
                        return Err(ExecutionError(
                            axum::http::StatusCode::NOT_FOUND,
                            "Selected conversation turn was not found in this workspace.",
                        )
                        .into());
                    };
                    Some(seq)
                }
                None => None,
            };

            let branch_title = match title {
                Some(custom) => {
                    let trimmed = custom.trim();
                    if trimmed.is_empty() || trimmed.chars().count() > 80 {
                        return Err(ExecutionError(
                            axum::http::StatusCode::BAD_REQUEST,
                            "Title must contain 1–80 characters.",
                        )
                        .into());
                    }
                    trimmed.to_owned()
                }
                None => {
                    let suffix = " (branch)";
                    let max_prefix = 80 - suffix.chars().count();
                    let prefix: String = source_title.chars().take(max_prefix).collect();
                    format!("{prefix}{suffix}")
                }
            };

            tx.pragma_update(None, "defer_foreign_keys", true)?;

            let new_workspace_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO workspaces(id, title, guest_id) VALUES (?1, ?2, ?3)",
                params![new_workspace_id, branch_title, guest_id],
            )?;

            let mut id_map = HashMap::new();
            id_map.insert(source_id.clone(), new_workspace_id.clone());

            for sql in [
                "SELECT id FROM runs WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT id FROM revisions WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT id FROM proposals WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT edit_id FROM proposals WHERE workspace_id=?1 AND edit_id IS NOT NULL ORDER BY rowid",
                "SELECT id FROM executions WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT re.execution_id FROM run_executions re JOIN runs r ON r.id=re.run_id WHERE r.workspace_id=?1 ORDER BY re.rowid",
                "SELECT id FROM assets WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT id FROM image_jobs WHERE workspace_id=?1 ORDER BY rowid",
                "SELECT id FROM image_assets WHERE workspace_id=?1 ORDER BY rowid",
            ] {
                register_ids(&tx, sql, &source_id, &mut id_map)?;
            }

            // Copy runs that either have kept messages or back workspace revisions, proposals, assets, executions, or images.
            let run_rows = tx
                .prepare(
                    "SELECT id,status,error,provider,model,output,stats,created_at_ms,started_at_ms,finished_at_ms,
                            mode,base_revision_id,context_messages,execution_result,search_config,search_activity,
                            model_rounds,execution_revision_id,node_scope,input_assets,image_config,image_input,import_enabled,workspace_tools,model_route,image_attachments
                     FROM runs
                     WHERE workspace_id=?1
                       AND (
                           EXISTS(SELECT 1 FROM messages m WHERE m.workspace_id=?1 AND m.run_id=runs.id AND (?2 IS NULL OR m.seq <= ?2))
                           OR EXISTS(SELECT 1 FROM proposals p WHERE p.workspace_id=?1 AND p.run_id=runs.id)
                           OR EXISTS(SELECT 1 FROM revisions rev WHERE rev.workspace_id=?1 AND rev.run_id=runs.id)
                           OR EXISTS(SELECT 1 FROM assets a WHERE a.workspace_id=?1 AND a.origin='import' AND json_extract(a.source,'$.run_id')=runs.id)
                           OR EXISTS(SELECT 1 FROM image_jobs ij WHERE ij.workspace_id=?1 AND ij.run_id=runs.id)
                           OR EXISTS(SELECT 1 FROM run_executions re WHERE re.run_id=runs.id)
                       )
                     ORDER BY rowid",
                )?
                .query_map(params![source_id, max_seq], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                        r.get::<_, Option<i64>>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                        r.get::<_, String>(10)?,
                        r.get::<_, Option<String>>(11)?,
                        r.get::<_, String>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, Option<String>>(14)?,
                        r.get::<_, String>(15)?,
                        r.get::<_, i64>(16)?,
                        r.get::<_, Option<String>>(17)?,
                        r.get::<_, String>(18)?,
                        r.get::<_, String>(19)?,
                        r.get::<_, Option<String>>(20)?,
                        r.get::<_, Option<String>>(21)?,
                        r.get::<_, bool>(22)?,
                        r.get::<_, bool>(23)?,
                        r.get::<_, Option<String>>(24)?,
                        r.get::<_, String>(25)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (
                id,
                status,
                error,
                provider,
                model,
                output,
                stats,
                created_at_ms,
                started_at_ms,
                finished_at_ms,
                mode,
                base_revision_id,
                context_messages,
                execution_result,
                search_config,
                search_activity,
                model_rounds,
                execution_revision_id,
                node_scope,
                input_assets,
                image_config,
                image_input,
                import_enabled,
                workspace_tools,
                model_route,
                image_attachments,
            ) in run_rows
            {
                tx.execute(
                    "INSERT INTO runs(
                        id,workspace_id,status,error,provider,model,output,stats,created_at_ms,started_at_ms,finished_at_ms,
                        mode,base_revision_id,context_messages,execution_result,search_config,search_activity,
                        model_rounds,execution_revision_id,node_scope,input_assets,image_config,image_input,import_enabled,workspace_tools,model_route,image_attachments
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        status,
                        error,
                        provider,
                        model,
                        output,
                        stats,
                        created_at_ms,
                        started_at_ms,
                        finished_at_ms,
                        mode,
                        map_opt_id(base_revision_id, &id_map)?,
                        context_messages,
                        remap_opt_json(execution_result, &id_map)?,
                        search_config,
                        search_activity,
                        model_rounds,
                        map_opt_id(execution_revision_id, &id_map)?,
                        node_scope,
                        remap_json(&input_assets, &id_map)?,
                        image_config,
                        remap_opt_json(image_input, &id_map)?,
                        import_enabled,
                        workspace_tools,
                        model_route,
                        remap_json(&image_attachments, &id_map)?,
                    ],
                )?;
            }

            // Copy messages up to max_seq (preserving order).
            let message_rows = tx
                .prepare(
                    "SELECT run_id, role, content FROM messages
                     WHERE workspace_id=?1 AND (?2 IS NULL OR seq <= ?2)
                     ORDER BY seq",
                )?
                .query_map(params![source_id, max_seq], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (run_id, role, content) in message_rows {
                tx.execute(
                    "INSERT INTO messages(workspace_id, run_id, role, content) VALUES (?1, ?2, ?3, ?4)",
                    params![new_workspace_id, map_id(&run_id, &id_map)?, role, content],
                )?;
            }

            // Copy executions and run_executions.
            let exec_rows = tx
                .prepare(
                    "SELECT id,revision_id,entry,action,code,input_assets,image_id,status,stdout,stderr,
                            exit_code,signal,truncated,error,created_at_ms,finished_at_ms,output_files
                     FROM executions WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<i64>>(10)?,
                        r.get::<_, Option<String>>(11)?,
                        r.get::<_, i64>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, i64>(14)?,
                        r.get::<_, Option<i64>>(15)?,
                        r.get::<_, String>(16)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (
                id,
                revision_id,
                entry,
                action,
                code,
                input_assets,
                image_id,
                status,
                stdout,
                stderr,
                exit_code,
                signal,
                truncated,
                error,
                created_at_ms,
                finished_at_ms,
                output_files,
            ) in exec_rows
            {
                tx.execute(
                    "INSERT INTO executions(
                        id,workspace_id,revision_id,entry,action,code,input_assets,image_id,status,stdout,stderr,
                        exit_code,signal,truncated,error,created_at_ms,finished_at_ms,output_files
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        map_opt_id(revision_id, &id_map)?,
                        entry,
                        action,
                        code,
                        remap_json(&input_assets, &id_map)?,
                        image_id,
                        status,
                        stdout,
                        stderr,
                        exit_code,
                        signal,
                        truncated,
                        error,
                        created_at_ms,
                        finished_at_ms,
                        output_files,
                    ],
                )?;
            }

            let run_exec_rows = tx
                .prepare(
                    "SELECT re.execution_id,re.run_id,re.seq,re.call_id,re.entry,re.action,re.code,re.input_assets,re.result
                     FROM run_executions re JOIN runs r ON r.id=re.run_id
                     WHERE r.workspace_id=?1 ORDER BY re.rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (execution_id, run_id, seq, call_id, entry, action, code, input_assets, result) in
                run_exec_rows
            {
                tx.execute(
                    "INSERT INTO run_executions(execution_id,run_id,seq,call_id,entry,action,code,input_assets,result)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        map_id(&execution_id, &id_map)?,
                        map_id(&run_id, &id_map)?,
                        seq,
                        call_id,
                        entry,
                        action,
                        code,
                        remap_json(&input_assets, &id_map)?,
                        remap_opt_json(result, &id_map)?,
                    ],
                )?;
            }

            // Copy workspace text assets.
            let asset_rows = tx
                .prepare(
                    "SELECT id,name,media_type,bytes,content,origin,execution_id,created_at_ms,source
                     FROM assets WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (id, name, media_type, bytes, content, origin, execution_id, created_at_ms, source) in
                asset_rows
            {
                tx.execute(
                    "INSERT INTO assets(id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms,source)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        name,
                        media_type,
                        bytes,
                        content,
                        origin,
                        map_opt_id(execution_id, &id_map)?,
                        created_at_ms,
                        remap_opt_json(source, &id_map)?,
                    ],
                )?;
            }

            // Copy image jobs and image assets.
            let image_job_rows = tx
                .prepare(
                    "SELECT id,run_id,call_id,prompt,seed,settings,status,remote_job_id,agent_context,
                            error,created_at_ms,finished_at_ms,progress,last_contact_at_ms
                     FROM image_jobs WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, i64>(10)?,
                        r.get::<_, Option<i64>>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        r.get::<_, Option<i64>>(13)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (
                id,
                run_id,
                call_id,
                prompt,
                seed,
                settings,
                status,
                remote_job_id,
                agent_context,
                error,
                created_at_ms,
                finished_at_ms,
                progress,
                last_contact_at_ms,
            ) in image_job_rows
            {
                tx.execute(
                    "INSERT INTO image_jobs(
                        id,workspace_id,run_id,call_id,prompt,seed,settings,status,remote_job_id,agent_context,
                        error,created_at_ms,finished_at_ms,progress,last_contact_at_ms
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        map_id(&run_id, &id_map)?,
                        call_id,
                        prompt,
                        seed,
                        settings,
                        status,
                        remote_job_id,
                        agent_context,
                        error,
                        created_at_ms,
                        finished_at_ms,
                        progress,
                        last_contact_at_ms,
                    ],
                )?;
            }

            let image_asset_rows = tx
                .prepare(
                    "SELECT id,image_job_id,name,origin,media_type,bytes,width,height,sha256,content,created_at_ms
                     FROM image_assets WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, Vec<u8>>(9)?,
                        r.get::<_, i64>(10)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (
                id,
                image_job_id,
                name,
                origin,
                media_type,
                bytes,
                width,
                height,
                sha256,
                content,
                created_at_ms,
            ) in image_asset_rows
            {
                tx.execute(
                    "INSERT INTO image_assets(
                        id,workspace_id,image_job_id,name,origin,media_type,bytes,width,height,sha256,content,created_at_ms
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        map_opt_id(image_job_id, &id_map)?,
                        name,
                        origin,
                        media_type,
                        bytes,
                        width,
                        height,
                        sha256,
                        content,
                        created_at_ms,
                    ],
                )?;
            }

            // Copy revisions and proposals (including pending unapplied proposals).
            let revision_rows = tx
                .prepare(
                    "SELECT id,run_id,execution_id,edit_id,asset_ids,number,html,files,asset_bindings
                     FROM revisions WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (id, run_id, execution_id, edit_id, asset_ids, number, html, files, asset_bindings) in
                revision_rows
            {
                tx.execute(
                    "INSERT INTO revisions(
                        id,workspace_id,run_id,execution_id,edit_id,asset_ids,number,html,files,asset_bindings
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        map_opt_id(run_id, &id_map)?,
                        map_opt_id(execution_id, &id_map)?,
                        map_opt_id(edit_id, &id_map)?,
                        remap_opt_json(asset_ids, &id_map)?,
                        number,
                        html,
                        files,
                        remap_json(&asset_bindings, &id_map)?,
                    ],
                )?;
            }

            let proposal_rows = tx
                .prepare(
                    "SELECT id,run_id,execution_id,edit_id,asset_ids,base_revision_id,status,changes,
                            applied_revision_id,rejection_reason,asset_changes
                     FROM proposals WHERE workspace_id=?1 ORDER BY rowid",
                )?
                .query_map([&source_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, String>(10)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for (
                id,
                run_id,
                execution_id,
                edit_id,
                asset_ids,
                base_revision_id,
                status,
                changes,
                applied_revision_id,
                rejection_reason,
                asset_changes,
            ) in proposal_rows
            {
                tx.execute(
                    "INSERT INTO proposals(
                        id,workspace_id,run_id,execution_id,edit_id,asset_ids,base_revision_id,status,changes,
                        applied_revision_id,rejection_reason,asset_changes
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    params![
                        map_id(&id, &id_map)?,
                        new_workspace_id,
                        map_opt_id(run_id, &id_map)?,
                        map_opt_id(execution_id, &id_map)?,
                        map_opt_id(edit_id, &id_map)?,
                        remap_opt_json(asset_ids, &id_map)?,
                        map_opt_id(base_revision_id, &id_map)?,
                        status,
                        changes,
                        map_opt_id(applied_revision_id, &id_map)?,
                        rejection_reason,
                        remap_json(&asset_changes, &id_map)?,
                    ],
                )?;
            }

            let fk_violation: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get(0),
            )?;
            if fk_violation {
                bail!("workspace branch failed foreign-key verification");
            }

            tx.commit()?;
            Ok(Some(Workspace {
                id: new_workspace_id,
                title: branch_title,
            }))
        })
        .await
    }
}
