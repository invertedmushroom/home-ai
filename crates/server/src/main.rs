use anyhow::Context;
use deep_server::{App, config::Config, router, store::Store};
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "model-calls") {
        let data_dir = std::env::var("DEEP_DATA_DIR").unwrap_or_else(|_| "data".into());
        let store =
            Store::open_for_maintenance(&std::path::Path::new(&data_dir).join("deep.sqlite3"))?;
        if args.len() != 1 {
            anyhow::ensure!(
                args.len() == 4 && args[1] == "--resolve" && args[3] == "--confirm-idle",
                "Usage: deep-server model-calls [--resolve <operation-id> --confirm-idle]"
            );
            store.resolve_model_call(args[2].clone()).await?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&store.model_calls().await?)?
        );
        return Ok(());
    }
    if !args.is_empty() {
        anyhow::ensure!(
            args[0] == "cleanup-guests",
            "Usage: deep-server cleanup-guests [--inactive-days 7] [--apply]"
        );
        let mut days = 7;
        let mut apply = false;
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--apply" => apply = true,
                "--inactive-days" => {
                    index += 1;
                    days = args
                        .get(index)
                        .context("--inactive-days requires a positive number")?
                        .parse::<u32>()?;
                }
                _ => {
                    anyhow::bail!("Usage: deep-server cleanup-guests [--inactive-days 7] [--apply]")
                }
            }
            index += 1;
        }
        let data_dir = std::env::var("DEEP_DATA_DIR").unwrap_or_else(|_| "data".into());
        let store =
            Store::open_for_maintenance(&std::path::Path::new(&data_dir).join("deep.sqlite3"))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&store.cleanup_guests(days, apply).await?)?
        );
        return Ok(());
    }
    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.bind).await.with_context(|| {
        format!("Cannot listen on {}; change DEEP_BIND or stop the server already using that address.", config.bind)
    })?;
    std::fs::create_dir_all(&config.data_dir)?;
    let store = Store::open(&config.data_dir.join("deep.sqlite3"))?;
    let app = App::new(config.clone(), store.clone())?;
    let telemetry = deep_server::telemetry::init(&config, store).await?;
    let worker = app.worker.clone();
    let images = app.images.clone();
    tokio::spawn(async move {
        if let Err(error) = images.recover().await {
            tracing::error!(%error,"image recovery could not start");
        }
    });
    tokio::spawn(async move {
        worker.status().await;
    });
    let shutdown_worker = app.worker.clone();
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Cannot register the SIGTERM handler")?;
    let app = router(app).layer(TraceLayer::new_for_http());
    tracing::info!(address=%listener.local_addr()?, backend=%config.default_backend, "Deep is ready");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            let signal = tokio::select! {
                result = tokio::signal::ctrl_c() => result,
                _ = terminate.recv() => Ok(()),
            };
            #[cfg(not(unix))]
            let signal = tokio::signal::ctrl_c().await;
            if let Err(error) = signal {
                tracing::error!(%error, "Shutdown signal handler failed");
            }
            shutdown_worker.shutdown().await;
        })
        .await;
    telemetry.shutdown().await;
    result?;
    Ok(())
}
