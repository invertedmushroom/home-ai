use super::{Store, now_ms};
use anyhow::Result;

pub(crate) struct TelemetrySnapshot {
    pub queued: u64,
    pub running: u64,
    pub unresolved: Vec<(String, u64, f64)>,
}

impl Store {
    pub(crate) async fn telemetry_snapshot(&self) -> Result<TelemetrySnapshot> {
        self.call(|connection| {
            let (queued, running) = connection.query_row(
                "SELECT COALESCE(SUM(status='queued'),0),COALESCE(SUM(status='running'),0) FROM runs WHERE status IN ('queued','running')",
                [], |row| Ok((row.get::<_,i64>(0)? as u64,row.get::<_,i64>(1)? as u64)),
            )?;
            let now = now_ms();
            let unresolved = connection.prepare(
                "SELECT resource_group,COUNT(*),MIN(created_at_ms) FROM (
                    SELECT resource_group,created_at_ms FROM model_calls WHERE status='unresolved'
                    UNION ALL
                    SELECT COALESCE(json_extract(r.image_config,'$.resource_group'),CASE WHEN json_extract(r.image_config,'$.provider')='openai' THEN 'openai-images' ELSE 'local-gpu' END),j.created_at_ms
                    FROM image_jobs j JOIN runs r ON r.id=j.run_id WHERE j.status='unresolved'
                ) GROUP BY resource_group",
            )?.query_map([], |row| {
                let created: i64 = row.get(2)?;
                Ok((row.get(0)?,row.get::<_,i64>(1)? as u64,now.saturating_sub(created).max(0) as f64 / 1000.0))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(TelemetrySnapshot { queued, running, unresolved })
        }).await
    }
}
