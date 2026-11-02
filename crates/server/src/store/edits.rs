use super::*;

#[derive(Debug)]
pub struct InvalidEdit(pub String);
impl std::fmt::Display for InvalidEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for InvalidEdit {}

impl Store {
    pub async fn propose_edits(
        &self,
        workspace: String,
        edit_id: String,
        base: Option<String>,
        changes: Vec<FileChange>,
        asset_changes: Vec<AssetChange>,
    ) -> Result<Option<Proposal>> {
        Uuid::parse_str(&edit_id).map_err(|_| InvalidEdit("Edit ID must be a UUID.".into()))?;
        self.call(move |c| {
            let tx = c.transaction()?;
            if let Some(proposal) = tx.query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE edit_id=?1"),
                [&edit_id], read_proposal,
            ).optional()? {
                if proposal.workspace_id != workspace || proposal.base_revision_id != base || proposal.changes != changes || proposal.asset_changes != asset_changes {
                    return Err(ArtifactConflict::EditIdentity.into());
                }
                return Ok(Some(proposal));
            }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)", [&workspace], |r| r.get(0))?;
            if !exists { return Ok(None); }
            if head(&tx, &workspace)?.as_ref().map(|r| &r.id) != base.as_ref() {
                return Err(ArtifactConflict::StaleBase.into());
            }
            let (files,bindings) = apply_project_changes(
                &base_files(&tx, &workspace, base.as_deref())?,
                &project_assets::base_bindings(&tx,&workspace,base.as_deref())?,
                &changes,&asset_changes,
            )
                .map_err(|error| InvalidEdit(error.to_string()))?;
            project_assets::resolve(&tx,&workspace,&files,&bindings)
                .map_err(|error|InvalidEdit(error.to_string()))?;
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO proposals(id,workspace_id,edit_id,base_revision_id,status,changes,asset_changes) VALUES (?1,?2,?3,?4,'pending',?5,?6)",
                params![id, workspace, edit_id, base, serde_json::to_string(&changes)?,serde_json::to_string(&asset_changes)?])?;
            let proposal = tx.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"), [id], read_proposal)?;
            tx.commit()?;
            Ok(Some(proposal))
        }).await
    }
}
