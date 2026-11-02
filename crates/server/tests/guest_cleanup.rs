use deep_server::store::Store;
use std::process::Command;

#[test]
fn cleanup_command_previews_only_after_the_server_releases_the_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let store = Store::open(&path).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_deep-server"))
            .env("DEEP_DATA_DIR", dir.path())
            .args(["cleanup-guests", "--inactive-days", "7"])
            .output()
            .unwrap()
    };
    let blocked = run();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("another Deep process"));
    drop(store);
    let preview = run();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&preview.stdout).unwrap(),
        serde_json::json!([])
    );
}
