//! 复用原子多范围计数器，为每次来源尝试分配独立租约身份。

use std::{sync::Arc, time::SystemTime};

use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use gateway_core::{
    engine::{
        provider::ResourceLease,
        source_admission::{SourceAdmissionError, SourceAdmissionPort, SourceAdmissionRequest},
    },
    lifecycle::CancellationToken,
    policy::RateLimits,
    task::{DaemonTask, WorkerTaskError},
};
use tokio::sync::{Mutex, mpsc, oneshot};

use super::{
    ClientAdmissionDecision, ClientAdmissionLimits, ClientAdmissionRepository,
    ClientAdmissionRequest, ClientAdmissionScope,
};

const QUEUE_CAPACITY: usize = 4096;
const MAX_IN_FLIGHT: usize = 128;

type AdmissionResult = Result<Box<dyn ResourceLease>, SourceAdmissionError>;

struct Acquisition {
    request: SourceAdmissionRequest,
    result: oneshot::Sender<AdmissionResult>,
    releases: mpsc::Sender<Release>,
}

struct Release {
    scopes: Vec<String>,
    lease_id: String,
}

struct SourceLease {
    release: Option<Release>,
    sender: mpsc::Sender<Release>,
}

impl Drop for SourceLease {
    fn drop(&mut self) {
        if let Some(release) = self.release.take()
            && self.sender.try_send(release).is_err()
        {
            tracing::warn!("来源租约释放队列不可用，依赖请求截止时间收敛");
        }
    }
}

#[derive(Clone)]
pub struct RedisSourceAdmissionPort {
    acquisitions: mpsc::Sender<Acquisition>,
    releases: mpsc::Sender<Release>,
}

impl RedisSourceAdmissionPort {
    #[must_use]
    pub fn new(repository: Arc<dyn ClientAdmissionRepository>) -> (Self, SourceAdmissionWorker) {
        let (acquisitions, requests) = mpsc::channel(QUEUE_CAPACITY);
        let (releases, returned) = mpsc::channel(QUEUE_CAPACITY);
        (
            Self {
                acquisitions,
                releases,
            },
            SourceAdmissionWorker {
                repository,
                requests: Mutex::new(requests),
                returned: Mutex::new(returned),
            },
        )
    }
}

impl SourceAdmissionPort for RedisSourceAdmissionPort {
    fn acquire(&self, request: SourceAdmissionRequest) -> BoxFuture<'_, AdmissionResult> {
        Box::pin(async move {
            let (result, receiver) = oneshot::channel();
            self.acquisitions
                .try_send(Acquisition {
                    request,
                    result,
                    releases: self.releases.clone(),
                })
                .map_err(|_| SourceAdmissionError::Unavailable)?;
            receiver
                .await
                .map_err(|_| SourceAdmissionError::Unavailable)?
        })
    }
}

/// Host 拥有生命周期；有界并发处理准入和释放，不在请求中创建脱离管理的任务。
pub struct SourceAdmissionWorker {
    repository: Arc<dyn ClientAdmissionRepository>,
    requests: Mutex<mpsc::Receiver<Acquisition>>,
    returned: Mutex<mpsc::Receiver<Release>>,
}

impl DaemonTask for SourceAdmissionWorker {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut requests = self.requests.lock().await;
            let mut returned = self.returned.lock().await;
            let mut pending: FuturesUnordered<BoxFuture<'_, ()>> = FuturesUnordered::new();
            loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        // 已送往 Redis 的操作先收敛，避免取消等待使后到的 grant 泄漏。
                        requests.close();
                        while requests.try_recv().is_ok() {}
                        while pending.next().await.is_some() {}
                        while let Ok(release) = returned.try_recv() {
                            release_lease(self.repository.as_ref(), release).await;
                        }
                        return Ok(());
                    }
                    Some(()) = pending.next(), if !pending.is_empty() => {}
                    Some(release) = returned.recv(), if pending.len() < MAX_IN_FLIGHT => {
                        pending.push(Box::pin(release_lease(self.repository.as_ref(), release)));
                    }
                    Some(acquisition) = requests.recv(), if pending.len() < MAX_IN_FLIGHT => {
                        pending.push(Box::pin(acquire_lease(self.repository.as_ref(), acquisition)));
                    }
                }
            }
        })
    }
}

fn scope(scope_ref: String, limits: RateLimits) -> ClientAdmissionScope {
    ClientAdmissionScope {
        scope_ref,
        limits: ClientAdmissionLimits {
            max_concurrency: limits.max_concurrency,
            requests_per_minute: limits.requests_per_minute,
        },
    }
}

async fn acquire_lease(repository: &dyn ClientAdmissionRepository, acquisition: Acquisition) {
    if acquisition.result.is_closed() {
        return;
    }
    let result = reserve(repository, &acquisition.request, acquisition.releases).await;
    // receiver 已因取消/截止而关闭时，send 返回租约并立即 drop，仍安排释放。
    let _ = acquisition.result.send(result);
}

async fn reserve(
    repository: &dyn ClientAdmissionRepository,
    request: &SourceAdmissionRequest,
    releases: mpsc::Sender<Release>,
) -> AdmissionResult {
    let lease_ttl = request
        .deadline
        .duration_since(SystemTime::now())
        .map_err(|_| SourceAdmissionError::Unavailable)?;
    if !request.limits.is_valid()
        || request
            .shared_quota
            .as_ref()
            .is_some_and(|(_, limits)| !limits.is_valid())
        || lease_ttl.as_millis() == 0
    {
        return Err(SourceAdmissionError::Unavailable);
    }
    let mut scopes = vec![scope(format!("source:{}", request.source), request.limits)];
    if let Some((id, limits)) = &request.shared_quota {
        scopes.push(scope(format!("quota:{id}"), *limits));
    }
    let lease_id = uuid::Uuid::new_v4().to_string();
    let release = Release {
        scopes: scopes.iter().map(|scope| scope.scope_ref.clone()).collect(),
        lease_id: lease_id.clone(),
    };
    let admission = ClientAdmissionRequest {
        model_request_id: lease_id,
        lease_ttl,
        scopes,
    };
    // 先完成 acquire 再释放；即使 Redis 返回不确定错误，也清理可能已提交的占用。
    let decision = repository.admit_client_request(&admission).await;
    let lease = SourceLease {
        release: Some(release),
        sender: releases,
    };
    match decision {
        Ok(ClientAdmissionDecision::Granted) if request.deadline > SystemTime::now() => {
            Ok(Box::new(lease))
        }
        Ok(ClientAdmissionDecision::Rejected(_)) => Err(SourceAdmissionError::Capacity),
        _ => Err(SourceAdmissionError::Unavailable),
    }
}

async fn release_lease(repository: &dyn ClientAdmissionRepository, release: Release) {
    if repository
        .release_client_request(&release.scopes, &release.lease_id)
        .await
        .is_err()
    {
        tracing::warn!("来源租约后台释放失败，依赖请求截止时间收敛");
    }
}
