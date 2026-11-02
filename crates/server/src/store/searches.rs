use super::{Store, read_json};
use crate::search::{SearchConfig, SearchReceipt, Source, valid_query, valid_source_url};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn close_pending(c: &Connection, id: &str, status: &str) -> Result<()> {
    let mut receipts: Vec<SearchReceipt> =
        c.query_row("SELECT search_activity FROM runs WHERE id=?1", [id], |r| {
            read_json(r, 0)
        })?;
    for receipt in &mut receipts {
        if receipt.status == "pending" {
            receipt.status = status.into();
            receipt.error = Some(
                "Run ended before a search result was recorded; it will not be replayed.".into(),
            );
        }
    }
    c.execute(
        "UPDATE runs SET search_activity=?2 WHERE id=?1",
        params![id, serde_json::to_string(&receipts)?],
    )?;
    Ok(())
}

impl Store {
    pub(crate) async fn search_config(&self, id: String) -> Result<Option<SearchConfig>> {
        self.call(move |c| {
            let raw: Option<String> =
                c.query_row("SELECT search_config FROM runs WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
            raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
                .transpose()
        })
        .await
    }

    pub(crate) async fn search_intents(
        &self,
        id: String,
        incoming: Vec<SearchReceipt>,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let existing: Option<Vec<SearchReceipt>> = tx.query_row("SELECT search_activity FROM runs WHERE id=?1 AND status='running' AND search_config IS NOT NULL", [&id], |r| read_json(r, 0)).optional()?;
            let mut receipts = existing.ok_or_else(|| anyhow::anyhow!("Search run is no longer active."))?;
            ensure!(!incoming.is_empty() && receipts.len() + incoming.len() <= 2, "Search budget exceeded.");
            for item in incoming {
                ensure!(valid_query(&item.query) && item.status == "pending" && item.sources.is_empty()
                    && !receipts.iter().any(|r| r.call_id == item.call_id || r.id == item.id), "Invalid search intent.");
                receipts.push(item);
            }
            tx.execute("UPDATE runs SET search_activity=?2 WHERE id=?1", params![id, serde_json::to_string(&receipts)?])?;
            tx.commit()?;
            Ok(())
        }).await
    }

    pub(crate) async fn settle_search(
        &self,
        id: String,
        receipt_id: String,
        result: std::result::Result<Vec<Source>, String>,
        elapsed_ms: i64,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let existing: Option<Vec<SearchReceipt>> = tx
                .query_row(
                    "SELECT search_activity FROM runs WHERE id=?1 AND status='running'",
                    [&id],
                    |r| read_json(r, 0),
                )
                .optional()?;
            let Some(mut receipts) = existing else {
                return Ok(());
            };
            let offset: usize = receipts.iter().map(|r| r.sources.len()).sum();
            let item = receipts
                .iter_mut()
                .find(|r| r.id == receipt_id && r.status == "pending")
                .ok_or_else(|| anyhow::anyhow!("Search receipt is no longer pending."))?;
            item.elapsed_ms = Some(elapsed_ms.max(0));
            match result {
                Ok(mut sources) => {
                    ensure!(
                        sources.len() <= 5
                            && sources.iter().all(|s| valid_source_url(&s.url)
                                && !s.title.is_empty()
                                && s.title.chars().count() <= 200
                                && s.snippet.chars().count() <= 700),
                        "Invalid captured search sources."
                    );
                    for (index, source) in sources.iter_mut().enumerate() {
                        source.id = format!("S{}", offset + index + 1);
                    }
                    item.status = "succeeded".into();
                    item.sources = sources;
                }
                Err(error) => {
                    item.status = "failed".into();
                    item.error = Some(error);
                }
            }
            tx.execute(
                "UPDATE runs SET search_activity=?2 WHERE id=?1",
                params![id, serde_json::to_string(&receipts)?],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
