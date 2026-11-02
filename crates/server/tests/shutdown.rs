#![cfg(unix)]

use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;

#[tokio::test]
async fn unix_signals_exit_cleanly_and_preserve_saved_workspaces() {
    for signal in ["INT", "TERM"] {
        let directory = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut child = Command::new(env!("CARGO_BIN_EXE_deep-server"))
            .current_dir(directory.path())
            .env("DEEP_BIND", address.to_string())
            .env("DEEP_DATA_DIR", directory.path())
            .env("DEEP_PROVIDER", "demo")
            .env("DEEP_DEFAULT_BACKEND", "demo")
            .env("DEEP_PUBLIC_ORIGIN", " ")
            .env("DEEP_WORKER_IMAGE", " ")
            .env("DEEP_MODEL_PROFILES_FILE", " ")
            .env("DEEP_IMAGE_PROFILES_FILE", " ")
            .env("OTEL_SDK_DISABLED", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let base = format!("http://{address}/api");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "Server exited before readiness."
                );
                if client
                    .get(format!("{base}/status"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let saved: Value = client
            .post(format!("{base}/workspaces"))
            .json(&json!({"title":format!("Preserved after SIG{signal}")}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let sent = Command::new("kill")
            .args(["-s", signal, &child.id().unwrap().to_string()])
            .status()
            .await
            .unwrap();
        assert!(sent.success());
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "SIG{signal} bypassed graceful shutdown: {status}"
        );
        let store =
            deep_server::store::Store::open(&directory.path().join("deep.sqlite3")).unwrap();
        let detail = store
            .workspace(saved["id"].as_str().unwrap().into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            detail.workspace.title,
            format!("Preserved after SIG{signal}")
        );
    }
}
