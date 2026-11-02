//! The only native execution seam. Docker is a trusted, explicitly selected local
//! control plane; project text never becomes a host command or a host mount.
use crate::store::{
    Execution, ExecutionError, ExecutionInput, ExecutionResult, ExecutionSpec, Store,
};
use anyhow::{Result, bail};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 65_536;
const CONTROL_SECONDS: u64 = 5;
const HOST_SECONDS: u64 = 20;

#[derive(Serialize)]
pub struct WorkerStatus {
    pub enabled: bool,
    pub ready: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct Worker {
    store: Store,
    image: Option<String>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    shutting_down: bool,
    recovered: bool,
    docker: Option<Docker>,
    tasks: HashMap<String, CancellationToken>,
}

#[derive(Clone)]
struct Docker {
    endpoint: String,
    daemon: String,
}

struct Output {
    ok: bool,
    stdout: Vec<u8>,
}

async fn bounded(mut source: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut source)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        bail!("Worker protocol output exceeded its budget.");
    }
    Ok(bytes)
}

async fn cli(
    endpoint: Option<&str>,
    args: &[&str],
    input: Option<Vec<u8>>,
    seconds: u64,
    limit: usize,
) -> Result<Output> {
    let mut command = Command::new("docker");
    command
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .env_remove("DOCKER_TLS_VERIFY")
        .env_remove("DOCKER_CERT_PATH");
    if let Some(endpoint) = endpoint {
        command.args(["--host", endpoint]);
    }
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("Docker CLI is unavailable."))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let work = async {
        let send = async move {
            if let Some(input) = input {
                stdin.write_all(&input).await?;
            }
            stdin.shutdown().await?;
            Ok::<_, anyhow::Error>(())
        };
        let (_, stdout, _, status) = tokio::try_join!(
            send,
            bounded(stdout, limit),
            bounded(stderr, limit),
            async { Ok::<_, anyhow::Error>(child.wait().await?) }
        )?;
        Ok(Output {
            ok: status.success(),
            stdout,
        })
    };
    tokio::time::timeout(Duration::from_secs(seconds), work)
        .await
        .map_err(|_| anyhow::anyhow!("Docker control operation timed out."))?
}

fn unavailable() -> anyhow::Error {
    ExecutionError(StatusCode::SERVICE_UNAVAILABLE, "Worker unavailable. Check the local Docker engine, worker image and data-directory binding.").into()
}

impl Docker {
    async fn command(&self, args: &[&str]) -> Result<Output> {
        cli(Some(&self.endpoint), args, None, CONTROL_SECONDS, 65_536).await
    }

    async fn check(&self) -> Result<()> {
        let output = self.command(&["info", "--format", "{{json .}}"]).await?;
        if !output.ok {
            bail!("Docker engine is unavailable.");
        }
        let info: Value = serde_json::from_slice(&output.stdout)?;
        if info["ID"].as_str() != Some(&self.daemon)
            || info["OSType"] != "linux"
            || ["MemoryLimit", "SwapLimit", "PidsLimit", "CpuCfsQuota"]
                .iter()
                .any(|key| info[key] != true)
            || !info["SecurityOptions"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|s| s.starts_with("name=seccomp")))
            })
        {
            bail!(
                "Worker requires its bound Linux Docker daemon with resource limits and seccomp."
            );
        }
        Ok(())
    }

    async fn image(&self, name: &str) -> Result<String> {
        self.check().await?;
        let output = self.command(&["image", "inspect", "--", name]).await?;
        if !output.ok {
            bail!("Build the configured worker image first.");
        }
        let value: Vec<Value> = serde_json::from_slice(&output.stdout)?;
        let image = value.first().ok_or_else(unavailable)?;
        if image["Config"]["Labels"]["deep.worker.protocol"] != "node-v2"
            || image["Config"]["Volumes"]
                .as_object()
                .is_some_and(|volumes| !volumes.is_empty())
        {
            bail!("Worker image has an incompatible protocol or declares volumes.");
        }
        let id = image["Id"]
            .as_str()
            .filter(|id| image_id(id))
            .ok_or_else(unavailable)?;
        Ok(id.into())
    }

    async fn inspect(&self, name: &str) -> Result<Option<Value>> {
        self.check().await?;
        let filter = format!("name=^/{name}$");
        let output = self
            .command(&[
                "container",
                "ls",
                "--all",
                "--no-trunc",
                "--filter",
                &filter,
                "--format",
                "{{.ID}}",
            ])
            .await?;
        if !output.ok {
            bail!("Cannot establish container absence.");
        }
        let text = std::str::from_utf8(&output.stdout)?.trim();
        if text.is_empty() {
            return Ok(None);
        }
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("Ambiguous worker container identity.");
        }
        let output = self.command(&["container", "inspect", text]).await?;
        if !output.ok {
            bail!("Cannot inspect worker container.");
        }
        let mut items: Vec<Value> = serde_json::from_slice(&output.stdout)?;
        if items.len() != 1 {
            bail!("Ambiguous worker container.");
        }
        Ok(items.pop())
    }

    async fn cleanup(&self, owner: &str, job: &str) -> Result<()> {
        let name = container_name(owner, job);
        if let Some(container) = self.inspect(&name).await? {
            if container["Config"]["Labels"]["deep.owner"] != owner
                || container["Config"]["Labels"]["deep.job"] != job
                || container["Name"] != format!("/{name}")
            {
                bail!("Container ownership mismatch.");
            }
            let id = container["Id"].as_str().ok_or_else(unavailable)?;
            let output = self
                .command(&["container", "rm", "--force", "--volumes", id])
                .await?;
            if !output.ok {
                bail!("Cannot remove worker container.");
            }
        }
        if self.inspect(&name).await?.is_some() {
            bail!("Worker cleanup is not confirmed.");
        }
        Ok(())
    }

    async fn sweep(&self, store: &Store) -> Result<()> {
        self.check().await?;
        let filter = format!("label=deep.owner={}", store.worker_owner);
        let output = self
            .command(&[
                "container",
                "ls",
                "--all",
                "--filter",
                &filter,
                "--format",
                "{{.Label \"deep.job\"}}",
            ])
            .await?;
        if !output.ok {
            bail!("Cannot enumerate owned worker containers.");
        }
        for id in std::str::from_utf8(&output.stdout)?.lines() {
            if uuid::Uuid::parse_str(id).is_ok() && store.owns_execution(id.into()).await? {
                self.cleanup(&store.worker_owner, id).await?;
            }
        }
        Ok(())
    }
}

fn image_id(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn container_name(owner: &str, id: &str) -> String {
    format!("deep-exec-{owner}-{id}")
}

fn local_endpoint(endpoint: &str) -> bool {
    if let Some(name) = endpoint.strip_prefix("npipe:////./pipe/") {
        return !name.is_empty()
            && name.len() <= 240
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    }
    endpoint.strip_prefix("unix://").is_some_and(|path| {
        path.starts_with('/')
            && !path.starts_with("//")
            && path.len() > 1
            && !path
                .chars()
                .any(|c| c.is_control() || matches!(c, '?' | '#'))
    })
}

impl Worker {
    pub fn new(store: Store, image: Option<String>) -> Self {
        Self {
            store,
            image,
            state: Arc::default(),
        }
    }

    async fn recover(&self, state: &mut State) -> Result<()> {
        if state.recovered {
            return Ok(());
        }
        if !state.tasks.is_empty() {
            bail!("Execution recovery cannot sweep live owned tasks.");
        }
        let binding = self.store.worker_binding().await?;
        if let Some(binding) = binding {
            if Some(binding.path.as_str()) != self.store.worker_path.as_deref()
                || !local_endpoint(&binding.endpoint)
            {
                bail!("Native worker ownership is bound to a different database path.");
            }
            let docker = Docker {
                endpoint: binding.endpoint,
                daemon: binding.daemon,
            };
            docker.sweep(&self.store).await?;
            for job in self.store.unfinished_executions().await? {
                docker.cleanup(&self.store.worker_owner, &job.id).await?;
                self.store
                    .finish_execution(
                        job.id,
                        ExecutionResult {
                            status: "interrupted",
                            error: Some(
                                "Server restarted; execution was stopped and was not replayed."
                                    .into(),
                            ),
                            ..Default::default()
                        },
                        true,
                    )
                    .await?;
            }
            state.docker = Some(docker);
        }
        state.recovered = true;
        Ok(())
    }

    async fn connect(&self, state: &mut State) -> Result<Docker> {
        if let Some(docker) = &state.docker {
            return Ok(docker.clone());
        }
        let context = cli(None, &["context", "show"], None, CONTROL_SECONDS, 4096).await?;
        if !context.ok {
            bail!("Cannot resolve Docker context.");
        }
        let name = std::str::from_utf8(&context.stdout)?.trim();
        let output = cli(
            None,
            &[
                "context",
                "inspect",
                "--format",
                "{{json .Endpoints.docker.Host}}",
                "--",
                name,
            ],
            None,
            CONTROL_SECONDS,
            4096,
        )
        .await?;
        if !output.ok {
            bail!("Cannot resolve Docker endpoint.");
        }
        let endpoint: String = serde_json::from_slice(&output.stdout)?;
        if !local_endpoint(&endpoint) {
            bail!("Only a local Unix socket or Windows named pipe Docker endpoint is supported.");
        }
        let output = cli(
            Some(&endpoint),
            &["info", "--format", "{{json .ID}}"],
            None,
            CONTROL_SECONDS,
            4096,
        )
        .await?;
        if !output.ok {
            bail!("Cannot resolve Docker daemon identity.");
        }
        let daemon: String = serde_json::from_slice(&output.stdout)?;
        if daemon.is_empty() {
            bail!("Missing Docker daemon identity.");
        }
        let docker = Docker {
            endpoint: endpoint.clone(),
            daemon: daemon.clone(),
        };
        docker.check().await?;
        self.store.bind_worker(endpoint, daemon).await?;
        state.docker = Some(docker.clone());
        Ok(docker)
    }

    pub async fn status(&self) -> WorkerStatus {
        let mut state = self.state.lock().await;
        let result = async {
            self.recover(&mut state).await?;
            let image = self.image.as_deref().ok_or_else(|| anyhow::anyhow!("Execution is disabled. Build the worker image and set DEEP_WORKER_IMAGE to enable it."))?;
            self.connect(&mut state).await?.image(image).await?;
            Ok::<_, anyhow::Error>(())
        }.await;
        if let Err(error) = &result
            && (self.image.is_some() || !state.recovered)
        {
            tracing::warn!(%error, "Worker check failed");
        }
        WorkerStatus { enabled: self.image.is_some(), ready: result.is_ok(), message: match result {
            Ok(()) => "Node worker ready: 15 s, 256 MiB, no external network or host mounts.".into(),
            Err(_) if self.image.is_none() && state.recovered => "Execution is disabled. Build the worker image and set DEEP_WORKER_IMAGE to enable it.".into(),
            Err(_) => "Worker unavailable. Check Docker, the worker image and the original database path. Pending cleanup blocks new executions.".into(),
        } }
    }

    pub async fn submit(&self, workspace: String, input: ExecutionInput) -> Result<Execution> {
        self.submit_inner(workspace, ExecutionSpec::Revision(input), None)
            .await
    }

    pub(crate) async fn submit_tool(
        &self,
        parent: String,
        execution_id: Option<String>,
    ) -> Result<Execution> {
        let worker = self.clone();
        // Admission owns its future through SQL binding and live-task registration.
        // Dropping a model/HTTP future cannot strand an admitted job between those steps.
        tokio::spawn(async move {
            let (workspace, spec) = worker
                .store
                .tool_execution_spec(parent.clone(), execution_id)
                .await?;
            worker.submit_inner(workspace, spec, Some(parent)).await
        })
        .await?
    }

    async fn submit_inner(
        &self,
        workspace: String,
        spec: ExecutionSpec,
        parent: Option<String>,
    ) -> Result<Execution> {
        let mut state = self.state.lock().await;
        if let Some(existing) = self.store.execution(spec.id().to_string()).await? {
            if !existing.matches_spec(&workspace, &spec) {
                return Err(ExecutionError(
                    StatusCode::CONFLICT,
                    "This execution ID already names different input.",
                )
                .into());
            }
            return Ok(existing);
        }
        if state.shutting_down {
            return Err(unavailable());
        }
        self.recover(&mut state).await.map_err(|_| unavailable())?;
        let image = self.image.as_deref().ok_or_else(unavailable)?;
        let docker = self.connect(&mut state).await.map_err(|_| unavailable())?;
        let image = docker.image(image).await.map_err(|_| unavailable())?;
        let (job, created) = self
            .store
            .admit_execution(workspace, spec, image, parent)
            .await?;
        if created {
            let token = CancellationToken::new();
            state.tasks.insert(job.id.clone(), token.clone());
            let worker = self.clone();
            let job = job.clone();
            tokio::spawn(async move {
                worker.execute(docker, job, token).await;
            });
        }
        Ok(job)
    }

    pub(crate) async fn cancel_run(&self, parent: String) -> Result<()> {
        for id in self.store.linked_execution_ids(parent).await? {
            self.cancel(id).await?;
        }
        Ok(())
    }

    pub async fn cancel(&self, id: String) -> Result<Option<Execution>> {
        let mut state = self.state.lock().await;
        let job = self.store.cancel_execution(id.clone()).await?;
        if let Some(token) = state.tasks.get(&id) {
            token.cancel();
        } else if job
            .as_ref()
            .is_some_and(|job| matches!(job.status.as_str(), "queued" | "running" | "cancelling"))
        {
            state.recovered = false;
            self.recover(&mut state).await.map_err(|_| unavailable())?;
        }
        self.store.execution(id).await
    }

    pub async fn shutdown(&self) {
        {
            let mut state = self.state.lock().await;
            state.shutting_down = true;
            for token in state.tasks.values() {
                token.cancel();
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if self.state.lock().await.tasks.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    async fn execute(&self, docker: Docker, job: Execution, token: CancellationToken) {
        let creating = AtomicBool::new(false);
        let work = async {
            if !self.store.start_execution(job.id.clone()).await? {
                bail!("Execution was cancelled before starting.");
            }
            let files = self.store.execution_files(job.id.clone()).await?;
            let input =
                serde_json::to_vec(&json!({"files":files,"entry":job.entry,"action":job.action}))?;
            let name = container_name(&self.store.worker_owner, &job.id);
            let owner_label = format!("deep.owner={}", self.store.worker_owner);
            let job_label = format!("deep.job={}", job.id);
            creating.store(true, Ordering::Relaxed);
            let output = docker
                .command(&[
                    "container",
                    "create",
                    "--pull=never",
                    "--interactive",
                    "--init=false",
                    "--name",
                    &name,
                    "--label",
                    &owner_label,
                    "--label",
                    &job_label,
                    "--network=none",
                    "--read-only",
                    "--user=0:0",
                    "--env=HTTP_PROXY=",
                    "--env=http_proxy=",
                    "--env=HTTPS_PROXY=",
                    "--env=https_proxy=",
                    "--env=FTP_PROXY=",
                    "--env=ftp_proxy=",
                    "--env=ALL_PROXY=",
                    "--env=all_proxy=",
                    "--env=NO_PROXY=",
                    "--env=no_proxy=",
                    "--cap-drop=ALL",
                    "--cap-add=SETUID",
                    "--cap-add=SETGID",
                    "--cap-add=KILL",
                    "--security-opt=no-new-privileges:true",
                    "--memory=256m",
                    "--memory-swap=256m",
                    "--cpus=1",
                    "--pids-limit=64",
                    "--shm-size=16m",
                    "--tmpfs=/work:rw,noexec,nosuid,nodev,size=16777216,mode=0777",
                    "--tmpfs=/output:rw,noexec,nosuid,nodev,size=16777216,mode=0777",
                    "--ipc=private",
                    "--cgroupns=private",
                    "--ulimit=nofile=256:256",
                    "--ulimit=core=0:0",
                    "--log-driver=none",
                    "--no-healthcheck",
                    "--restart=no",
                    "--workdir=/work",
                    "--entrypoint=/usr/local/bin/node",
                    &job.image_id,
                    "/opt/deep/runner.cjs",
                ])
                .await?;
            if !output.ok {
                bail!("Cannot create worker container.");
            }
            let id = std::str::from_utf8(&output.stdout)?.trim();
            if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("Invalid worker container identity.");
            }
            creating.store(false, Ordering::Relaxed);
            if token.is_cancelled() {
                bail!("Execution was cancelled before starting.");
            }
            let output = cli(
                Some(&docker.endpoint),
                &["container", "start", "--attach", "--interactive", id],
                Some(input),
                HOST_SECONDS,
                8_388_608,
            )
            .await?;
            // A watchdog exit has no protocol result. Docker start -a can return zero
            // independently of the container's exit code, so inspect actual state.
            let container = docker.inspect(&name).await?.ok_or_else(unavailable)?;
            if container["State"]["Running"] != false {
                bail!("Worker did not stop.");
            }
            if container["State"]["ExitCode"] == 124 {
                return Ok(ExecutionResult {
                    status: "timed_out",
                    error: Some("Execution exceeded its 15-second container deadline.".into()),
                    ..Default::default()
                });
            }
            if !output.ok || container["State"]["ExitCode"] != 0 {
                bail!("Worker failed or exhausted its resource budget.");
            }
            decode(&output.stdout)
        };
        let outcome = tokio::select! {
            biased;
            _ = token.cancelled() => Ok(ExecutionResult { status:"cancelled", ..Default::default() }),
            result = tokio::time::timeout(Duration::from_secs(HOST_SECONDS), work) => match result {
                Ok(value) => value,
                Err(_) => Ok(ExecutionResult { status:"timed_out", error:Some("Execution exceeded its host deadline.".into()), ..Default::default() }),
            },
        };
        let mut result = outcome.unwrap_or_else(|error| {
            tracing::warn!(%error, execution=%job.id, "Worker execution failed");
            ExecutionResult {
                status: "failed",
                error: Some(
                    "Worker failed. Check the entry, resource limits and Docker availability."
                        .into(),
                ),
                ..Default::default()
            }
        });
        if creating.load(Ordering::Relaxed) {
            result.error = Some("Creation was interrupted before code could start. A stopped container may remain until worker recovery.".into());
        }
        let cleaned = docker
            .cleanup(&self.store.worker_owner, &job.id)
            .await
            .is_ok();
        // Admission and recovery observe durable settlement and runtime ownership
        // together. Publishing a terminal row before removing this task permits a
        // new job to race a later recovery sweep from the old task.
        let mut state = self.state.lock().await;
        let settled = self
            .store
            .finish_execution(job.id.clone(), result, cleaned)
            .await;
        if let Err(error) = &settled {
            tracing::error!(%error, "Cannot persist execution settlement");
        }
        state.tasks.remove(&job.id);
        if !cleaned || creating.load(Ordering::Relaxed) || settled.is_err() {
            state.recovered = false;
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    exit_code: Option<i32>,
    signal: Option<String>,
    stdout: String,
    stderr: String,
    truncated: bool,
    files: Vec<crate::artifacts::File>,
    artifact_error: Option<String>,
}

fn decode(bytes: &[u8]) -> Result<ExecutionResult> {
    let mut report: Report = serde_json::from_slice(bytes)?;
    crate::artifacts::validate_files(&report.files)?;
    if report
        .exit_code
        .is_some_and(|code| !(0..=255).contains(&code))
        || report.signal.as_ref().is_some_and(|s| s.len() > 32)
        || report.stdout.len() > OUTPUT_LIMIT * 3
        || report.stderr.len() > OUTPUT_LIMIT * 3
        || report
            .artifact_error
            .as_ref()
            .is_some_and(|error| error.len() > 256)
    {
        bail!("Invalid worker observations.");
    }
    for text in [&mut report.stdout, &mut report.stderr] {
        if text.len() > OUTPUT_LIMIT {
            let mut end = OUTPUT_LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            report.truncated = true;
        }
    }
    Ok(ExecutionResult {
        status: if report.exit_code == Some(0) && report.artifact_error.is_none() {
            "succeeded"
        } else {
            "failed"
        },
        stdout: report.stdout,
        stderr: report.stderr,
        exit_code: report.exit_code,
        signal: report.signal,
        truncated: report.truncated,
        error: report.artifact_error,
        output_files: report.files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_or_ambiguous_docker_endpoints_are_rejected() {
        for value in [
            "npipe:////remote-host/pipe/docker_engine",
            "npipe:////localhost/pipe/docker_engine",
            "npipe:////./pipe/",
            "npipe:////./pipe/x/../y",
            "unix://remote/socket",
            "unix:////remote/socket",
            "tcp://127.0.0.1:2375",
            "ssh://local",
            "unix:///tmp/sock?x",
        ] {
            assert!(!local_endpoint(value), "{value}");
        }
        assert!(local_endpoint("npipe:////./pipe/dockerDesktopLinuxEngine"));
        assert!(local_endpoint("unix:///var/run/docker.sock"));
    }
    #[test]
    fn invalid_worker_reports_cannot_publish_success() {
        for value in [
            r#"{"exit_code":0}"#,
            r#"{"exit_code":256,"signal":null,"stdout":"","stderr":"","truncated":false,"files":[],"artifact_error":null}"#,
            r#"{"exit_code":0,"signal":null,"stdout":"","stderr":"","truncated":false,"files":[],"artifact_error":null,"extra":true}"#,
        ] {
            assert!(decode(value.as_bytes()).is_err());
        }
        let result=decode(br#"{"exit_code":1,"signal":null,"stdout":"result","stderr":"failed","truncated":false,"files":[],"artifact_error":null}"#).unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(result.stderr, "failed");
    }
}
