use crate::store::Store;
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) const LOCAL_GROUP: &str = "local-gpu";
const MAX_WAITERS: usize = crate::MAX_PENDING_AND_ACTIVE;

pub(crate) fn valid_group(group: &str) -> bool {
    (1..=64).contains(&group.len())
        && group
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    async fn queued(resources: &Resources, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let remaining = resources
                    .0
                    .lock()
                    .unwrap()
                    .get("shared")
                    .unwrap()
                    .waiters
                    .available_permits();
                if remaining == MAX_WAITERS - count {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fifo_waiters_cancel_cleanly_and_other_groups_progress() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let held = store
            .resources
            .acquire(&store, "shared", None, None)
            .await
            .unwrap();
        let (send, mut recv) = mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        let mut releases = Vec::new();
        for index in 0..3 {
            let owned = store.clone();
            let send = send.clone();
            let (release, wait) = oneshot::channel();
            releases.push(release);
            tasks.push(tokio::spawn(async move {
                let _permit = owned
                    .resources
                    .acquire(&owned, "shared", None, None)
                    .await
                    .unwrap();
                send.send(index).unwrap();
                let _ = wait.await;
            }));
            queued(&store.resources, index + 1).await;
        }
        tasks[1].abort();
        queued(&store.resources, 2).await;
        let independent = store
            .resources
            .acquire(&store, "independent", None, None)
            .await
            .unwrap();
        drop(independent);
        drop(held);
        assert_eq!(recv.recv().await, Some(0));
        releases.remove(0).send(()).unwrap();
        assert_eq!(recv.recv().await, Some(2));
        releases.pop().unwrap().send(()).unwrap();
        for task in tasks {
            let _ = task.await;
        }
        assert!(recv.try_recv().is_err());
    }

    #[tokio::test]
    async fn queue_is_bounded_and_dropped_waiters_restore_capacity() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let held = store
            .resources
            .acquire(&store, "shared", None, None)
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for _ in 0..MAX_WAITERS {
            let owned = store.clone();
            tasks.push(tokio::spawn(async move {
                owned.resources.acquire(&owned, "shared", None, None).await
            }));
        }
        queued(&store.resources, MAX_WAITERS).await;
        assert!(
            store
                .resources
                .acquire(&store, "shared", None, None)
                .await
                .is_err()
        );
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        queued(&store.resources, 0).await;
        drop(held);
        assert!(
            store
                .resources
                .acquire(&store, "shared", None, None)
                .await
                .is_ok()
        );
    }
}

struct Group {
    slot: Arc<Semaphore>,
    waiters: Arc<Semaphore>,
}

#[derive(Clone, Default)]
pub(crate) struct Resources(Arc<Mutex<HashMap<String, Arc<Group>>>>);

impl Resources {
    pub(crate) fn snapshot(&self) -> Vec<(String, u64, u64)> {
        self.0
            .lock()
            .expect("resource groups lock")
            .iter()
            .map(|(name, group)| {
                (
                    name.clone(),
                    (1 - group.slot.available_permits()) as u64,
                    (MAX_WAITERS - group.waiters.available_permits()) as u64,
                )
            })
            .collect()
    }

    #[tracing::instrument(target = "deep_server::telemetry", name = "resource.wait", skip_all,
        fields(resource_group=group, group_wait_ms=tracing::field::Empty, outcome=tracing::field::Empty))]
    pub(crate) async fn acquire(
        &self,
        store: &Store,
        group: &str,
        runtime: Option<&str>,
        image_job: Option<&str>,
    ) -> Result<OwnedSemaphorePermit> {
        ensure!(
            valid_group(group),
            "Resource group must contain 1–64 ASCII letters, digits, dots, underscores or hyphens."
        );
        let gate = self
            .0
            .lock()
            .expect("resource groups lock")
            .entry(group.to_owned())
            .or_insert_with(|| {
                Arc::new(Group {
                    slot: Arc::new(Semaphore::new(1)),
                    waiters: Arc::new(Semaphore::new(MAX_WAITERS)),
                })
            })
            .clone();
        let queued = gate.waiters.clone().try_acquire_owned().map_err(|_| {
            crate::telemetry::resource_rejected(group, "queue_full");
            tracing::Span::current().record("outcome", "queue_full");
            anyhow::anyhow!("Resource queue is full; wait and retry the request.")
        })?;
        let started = Instant::now();
        // Tokio's semaphore preserves FIFO order. Dropping this future removes
        // its waiter, while an owned operation retains the returned permit.
        let permit = gate.slot.clone().acquire_owned().await?;
        drop(queued);
        store
            .check_resource(
                group.to_owned(),
                runtime.map(str::to_owned),
                image_job.map(str::to_owned),
            )
            .await
            .inspect_err(|_| {
                crate::telemetry::resource_rejected(group, "ownership_blocked");
                tracing::Span::current().record("outcome", "ownership_blocked");
            })?;
        crate::telemetry::resource_acquired(group, started.elapsed());
        tracing::Span::current().record("group_wait_ms", started.elapsed().as_millis() as u64);
        tracing::Span::current().record("outcome", "acquired");
        tracing::info!(
            resource_group = group,
            group_wait_ms = started.elapsed().as_millis() as u64,
            "resource stage acquired"
        );
        Ok(permit)
    }
}
