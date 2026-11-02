use super::*;

#[derive(Debug, Serialize)]
pub struct GuestCleanup {
    pub guest: String,
    pub last_seen_at_ms: i64,
    pub workspaces: usize,
    pub blocked: bool,
    pub deleted: bool,
}

impl Store {
    pub(crate) async fn create_guest(&self, guest: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO guests(id,created_at_ms,last_seen_at_ms) VALUES(?1,?2,?2)",
                params![guest, now_ms()],
            )?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn touch_guest(&self, guest: String) -> Result<bool> {
        self.call(move |c| {
            let now = now_ms();
            c.execute(
                "UPDATE guests SET last_seen_at_ms=?2 WHERE id=?1 AND last_seen_at_ms < ?2-60000",
                params![guest, now],
            )?;
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM guests WHERE id=?1)",
                [guest],
                |row| row.get(0),
            )?)
        })
        .await
    }

    pub(crate) async fn guest_workspaces(&self, guest: String) -> Result<Vec<Workspace>> {
        self.call(move |c| {
            Ok(
                c.prepare("SELECT id,title FROM workspaces WHERE guest_id=?1 ORDER BY rowid DESC")?
                    .query_map([guest], |row| {
                        Ok(Workspace {
                            id: row.get(0)?,
                            title: row.get(1)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )
        })
        .await
    }

    pub(crate) async fn create_guest_workspace(
        &self,
        title: String,
        guest: String,
    ) -> Result<Workspace> {
        self.call(move |c| {
            let workspace = Workspace {
                id: Uuid::new_v4().to_string(),
                title,
            };
            c.execute(
                "INSERT INTO workspaces(id,title,guest_id) VALUES(?1,?2,?3)",
                params![workspace.id, workspace.title, guest],
            )?;
            Ok(workspace)
        })
        .await
    }

    pub(crate) async fn guest_owns(
        &self,
        guest: String,
        table: &'static str,
        id: String,
    ) -> Result<bool> {
        // Table names come only from the closed HTTP route inventory.
        ensure!(
            [
                "workspaces",
                "runs",
                "proposals",
                "assets",
                "image_assets",
                "image_jobs",
                "executions"
            ]
            .contains(&table),
            "Unknown owned resource."
        );
        self.call(move |c| {
            let sql = if table == "workspaces" {
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1 AND guest_id=?2)".to_owned()
            } else {
                format!("SELECT EXISTS(SELECT 1 FROM {table} resource JOIN workspaces w ON w.id=resource.workspace_id WHERE resource.id=?1 AND w.guest_id=?2)")
            };
            Ok(c.query_row(&sql, params![id,guest], |row| row.get(0))?)
        }).await
    }

    pub async fn cleanup_guests(
        &self,
        inactive_days: u32,
        apply: bool,
    ) -> Result<Vec<GuestCleanup>> {
        ensure!(inactive_days > 0, "--inactive-days must be positive.");
        self.call(move |c| {
            let cutoff = now_ms().saturating_sub(i64::from(inactive_days) * 86_400_000);
            let tx = c.transaction()?;
            let guests = tx.prepare("SELECT id,last_seen_at_ms FROM guests WHERE last_seen_at_ms < ?1 ORDER BY last_seen_at_ms")?
                .query_map([cutoff], |row| Ok((row.get::<_,String>(0)?, row.get::<_,i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut report = Vec::new();
            for (guest, last_seen_at_ms) in guests {
                let workspaces = tx.prepare("SELECT id FROM workspaces WHERE guest_id=?1")?
                    .query_map([&guest], |row| row.get::<_,String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut blocked = false;
                for workspace in &workspaces { blocked |= workspace_busy(&tx, workspace)?; }
                if apply && !blocked {
                    for workspace in &workspaces { delete_workspace(&tx, workspace)?; }
                    tx.execute("DELETE FROM guests WHERE id=?1", [&guest])?;
                }
                report.push(GuestCleanup { guest:guest.chars().take(12).collect(), last_seen_at_ms, workspaces:workspaces.len(), blocked, deleted:apply && !blocked });
            }
            tx.commit()?;
            Ok(report)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{HeaderMap, Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    const ORIGIN: &str = "https://demo.friend.ts.net";

    fn demo(store: Store) -> Router {
        crate::router(
            crate::App::new(
                crate::config::Config {
                    model_catalog: Default::default(),
                    bind: "127.0.0.1:0".parse().unwrap(),
                    public_origin: Some(ORIGIN.into()),
                    data_dir: ".".into(),
                    web_dir: ".".into(),
                    default_backend: "demo".into(),
                    worker_image: None,
                    search: None,
                    image_profiles: vec![],
                    backends: vec![crate::config::BackendConfig {
                        kind: crate::config::BackendKind::Demo,
                        runtime: None,
                        resource_group: None,
                        id: "demo".into(),
                        base_url: String::new(),
                        model: "demo-model".into(),
                        api_key: None,
                        timeout: std::time::Duration::from_secs(3),
                        max_output_tokens: 128,
                    }],
                },
                store,
            )
            .unwrap(),
        )
    }

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        cookie: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", "demo.friend.ts.net")
            .header("Content-Type", "application/json");
        if !cookie.is_empty() {
            request = request.header("Cookie", cookie);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, headers, value)
    }

    async fn guest(app: &Router) -> String {
        let (status, headers, body) = call(
            app,
            "POST",
            "/api/session",
            "",
            &[("Origin", ORIGIN)],
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let cookie = headers["set-cookie"].to_str().unwrap();
        for flag in [
            "__Host-deep_guest=",
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            "Path=/",
        ] {
            assert!(cookie.contains(flag));
        }
        assert!(body.get("token").is_none());
        cookie.split(';').next().unwrap().into()
    }

    #[tokio::test]
    async fn guest_credentials_require_bootstrap_and_cross_origin_requests_cannot_act() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let app = demo(store.clone());
        assert_eq!(
            call(&app, "GET", "/api/workspaces", "", &[], json!(null))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let cookie = guest(&app).await;
        let (_, headers, _) = call(
            &app,
            "POST",
            "/api/session",
            &cookie,
            &[("Origin", ORIGIN)],
            json!(null),
        )
        .await;
        assert!(!headers.contains_key("set-cookie"));
        for headers in [
            vec![("Origin", "https://other.ts.net")],
            vec![("Origin", "null")],
            vec![],
        ] {
            assert_eq!(
                call(
                    &app,
                    "POST",
                    "/api/workspaces",
                    &cookie,
                    &headers,
                    json!({"title":"forbidden"})
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
        for path in ["/api/worker", "/api/workspaces"] {
            assert_eq!(
                call(
                    &app,
                    "GET",
                    path,
                    &cookie,
                    &[("sec-fetch-site", "cross-site")],
                    json!(null)
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            call(&app, "GET", "/api/worker", &cookie, &[], json!(null))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/new-unclassified-route",
                &cookie,
                &[],
                json!(null)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert!(store.workspaces().await.unwrap().is_empty());
        let (_, headers, _) = call(&app, "GET", "/api/workspaces", &cookie, &[], json!(null)).await;
        assert_eq!(headers["cache-control"], "private, no-store");
    }

    #[tokio::test]
    async fn every_owned_resource_and_nested_asset_reference_stays_with_its_guest() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let local = store
            .create_workspace("operator data".into())
            .await
            .unwrap();
        let app = demo(store.clone());
        let a = guest(&app).await;
        let b = guest(&app).await;
        let w = call(
            &app,
            "POST",
            "/api/workspaces",
            &a,
            &[("Origin", ORIGIN)],
            json!({"title":"A"}),
        )
        .await
        .2["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other = call(
            &app,
            "POST",
            "/api/workspaces",
            &b,
            &[("Origin", ORIGIN)],
            json!({"title":"B"}),
        )
        .await
        .2["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let ids: Vec<String> = (0..6).map(|_| Uuid::new_v4().to_string()).collect();
        let seed_ids = ids.clone();
        let seed_workspace = w.clone();
        store.call(move |c| {
            let [run,asset,image,job,execution,proposal] = seed_ids.as_slice() else { unreachable!() };
            let profile = json!({"id":"test","label":"Test","endpoint":"http://127.0.0.1:9/mcp","settings":{"model_type":"flux2_klein_4b","resolution":"512x512","num_inference_steps":4}}).to_string();
            c.execute("INSERT INTO runs(id,workspace_id,status,provider,model,image_config) VALUES(?1,?2,'failed','demo','demo-model',?3)", params![run,seed_workspace,profile])?;
            c.execute("INSERT INTO assets(id,workspace_id,name,media_type,bytes,content,origin,created_at_ms) VALUES(?1,?2,'secret','text/plain',4,'hide','upload',0)", params![asset,seed_workspace])?;
            c.execute("INSERT INTO image_assets(id,workspace_id,name,origin,media_type,bytes,width,height,sha256,content,created_at_ms) VALUES(?1,?2,'secret.png','upload','image/png',1,1,1,?3,X'00',0)", params![image,seed_workspace,"0".repeat(64)])?;
            c.execute("INSERT INTO image_jobs(id,workspace_id,run_id,call_id,prompt,seed,settings,status,error,created_at_ms) VALUES(?1,?2,?3,'call','secret',0,'{}','unresolved','unknown',0)", params![job,seed_workspace,run])?;
            c.execute("INSERT INTO executions(id,workspace_id,entry,action,code,image_id,status,created_at_ms,finished_at_ms) VALUES(?1,?2,'main.js','run','','image','failed',0,1)", params![execution,seed_workspace])?;
            c.execute("INSERT INTO proposals(id,workspace_id,run_id,status,changes) VALUES(?1,?2,?3,'pending','[]')", params![proposal,seed_workspace,run])?;
            Ok(())
        }).await.unwrap();
        let [run, asset, image, job, execution, proposal] = ids.as_slice() else {
            unreachable!()
        };
        for (method, path) in [
            ("GET", format!("/api/workspaces/{w}")),
            ("DELETE", format!("/api/workspaces/{w}")),
            ("POST", format!("/api/workspaces/{w}/runs")),
            ("POST", format!("/api/workspaces/{w}/assets")),
            ("POST", format!("/api/workspaces/{w}/proposals")),
            ("POST", format!("/api/workspaces/{w}/asset-proposals")),
            ("POST", format!("/api/workspaces/{w}/image-assets")),
            ("POST", format!("/api/workspaces/{w}/project-export")),
            ("GET", format!("/api/workspaces/{w}/executions")),
            ("POST", format!("/api/workspaces/{w}/executions")),
            ("GET", format!("/api/runs/{run}")),
            ("DELETE", format!("/api/runs/{run}")),
            ("GET", format!("/api/assets/{asset}")),
            ("GET", format!("/api/image-assets/{image}")),
            (
                "GET",
                format!("/api/image-assets/{image}/content?download=true"),
            ),
            ("GET", format!("/api/image-jobs/{job}")),
            ("POST", format!("/api/image-jobs/{job}/reconcile")),
            ("POST", format!("/api/image-jobs/{job}/confirm-stopped")),
            ("POST", format!("/api/image-jobs/{job}/accept-unknown")),
            ("GET", format!("/api/executions/{execution}")),
            ("DELETE", format!("/api/executions/{execution}")),
            ("POST", format!("/api/executions/{execution}/proposal")),
            ("POST", format!("/api/proposals/{proposal}/apply")),
            ("POST", format!("/api/proposals/{proposal}/reject")),
            ("DELETE", format!("/api/proposals/{proposal}")),
            ("GET", format!("/api/workspaces/{}", local.id)),
        ] {
            assert_eq!(
                call(&app, method, &path, &b, &[("Origin", ORIGIN)], json!({}))
                    .await
                    .0,
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
        let (_, _, list) = call(&app, "GET", "/api/workspaces", &b, &[], json!(null)).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], other);
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/api/assets/{asset}"),
                &a,
                &[],
                json!(null)
            )
            .await
            .2["content"],
            "hide"
        );
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/workspaces/{other}/asset-proposals"),
                &b,
                &[("Origin", ORIGIN)],
                json!({"base_revision":{"id":null},"asset_ids":[asset]})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/workspaces/{other}/project-export"),
                &b,
                &[("Origin", ORIGIN)],
                json!({"files":[],"asset_bindings":[{"path":"secret.png","asset_id":image}]})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/image-jobs/{job}/confirm-stopped"),
                &a,
                &[("Origin", ORIGIN)],
                json!({"confirm_remote_stopped":true})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/image-jobs/{job}/accept-unknown"),
                &a,
                &[("Origin", ORIGIN)],
                json!({"accept_unknown_outcome":true})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert!(store.workspace(w).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cleanup_previews_without_recovery_and_skips_whole_busy_guests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let stale = "a".repeat(64);
        let busy = "b".repeat(64);
        let fresh = "c".repeat(64);
        for guest in [&stale, &busy, &fresh] {
            store.create_guest(guest.clone()).await.unwrap();
        }
        let old = store
            .create_guest_workspace("old".into(), stale.clone())
            .await
            .unwrap();
        let blocked = store
            .create_guest_workspace("busy".into(), busy.clone())
            .await
            .unwrap();
        let sibling = store
            .create_guest_workspace("keep sibling".into(), busy.clone())
            .await
            .unwrap();
        let recent = store
            .create_guest_workspace("recent".into(), fresh)
            .await
            .unwrap();
        let local = store.create_workspace("operator".into()).await.unwrap();
        let blocked_id = blocked.id.clone();
        store.call(move |c| {
            c.execute("UPDATE guests SET last_seen_at_ms=0 WHERE id IN (?1,?2)", params![stale,busy])?;
            c.execute("INSERT INTO runs(id,workspace_id,status,provider,model) VALUES('active',?1,'queued','demo','demo-model')", [blocked_id])?;
            Ok(())
        }).await.unwrap();
        assert!(Store::open_for_maintenance(&path).is_err());
        drop(store);
        let store = Store::open_for_maintenance(&path).unwrap();
        let report = store.cleanup_guests(7, false).await.unwrap();
        assert_eq!(report.len(), 2);
        assert!(report.iter().all(|guest| !guest.deleted));
        assert_eq!(report.iter().filter(|guest| guest.blocked).count(), 1);
        assert_eq!(
            store.run("active".into()).await.unwrap().unwrap().status,
            "queued"
        );
        assert!(store.workspace(old.id.clone()).await.unwrap().is_some());
        let report = store.cleanup_guests(7, true).await.unwrap();
        assert_eq!(report.iter().filter(|guest| guest.deleted).count(), 1);
        assert!(store.workspace(old.id).await.unwrap().is_none());
        for workspace in [blocked, sibling, recent, local] {
            assert!(store.workspace(workspace.id).await.unwrap().is_some());
        }
        assert_eq!(
            store.run("active".into()).await.unwrap().unwrap().status,
            "queued"
        );
    }

    #[tokio::test]
    async fn cleanup_preserves_pending_image_and_execution_cleanup_and_rolls_back_failures() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        for (guest, pending) in [("a", "image"), ("b", "execution"), ("c", "none")] {
            let owner = guest.repeat(64);
            store.create_guest(owner.clone()).await.unwrap();
            let workspace = store
                .create_guest_workspace(pending.into(), owner.clone())
                .await
                .unwrap();
            store.call(move |c| {
                c.execute("UPDATE guests SET last_seen_at_ms=0 WHERE id=?1", [owner])?;
                if pending == "image" {
                    c.execute("INSERT INTO runs(id,workspace_id,status,provider,model) VALUES('image_parent',?1,'failed','demo','demo-model')", [&workspace.id])?;
                    c.execute("INSERT INTO image_jobs(id,workspace_id,run_id,call_id,prompt,seed,settings,status,created_at_ms) VALUES('image',?1,'image_parent','call','p',0,'{}','unresolved',0)", [&workspace.id])?;
                } else if pending == "execution" {
                    c.execute("INSERT INTO executions(id,workspace_id,entry,action,code,image_id,status,created_at_ms) VALUES('execution',?1,'main.js','run','', 'image','cancelling',0)", [&workspace.id])?;
                }
                Ok(())
            }).await.unwrap();
        }
        let report = store.cleanup_guests(1, false).await.unwrap();
        assert_eq!(report.iter().filter(|guest| guest.blocked).count(), 2);
        store.call(|c| {
            c.execute_batch("CREATE TRIGGER refuse_cleanup BEFORE DELETE ON guests BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;")?;
            Ok(())
        }).await.unwrap();
        assert!(store.cleanup_guests(1, true).await.is_err());
        assert_eq!(store.workspaces().await.unwrap().len(), 3);
        store
            .call(|c| {
                c.execute_batch("DROP TRIGGER refuse_cleanup;")?;
                Ok(())
            })
            .await
            .unwrap();
        let report = store.cleanup_guests(1, true).await.unwrap();
        assert_eq!(report.iter().filter(|guest| guest.deleted).count(), 1);
        assert_eq!(store.workspaces().await.unwrap().len(), 2);
    }
}
