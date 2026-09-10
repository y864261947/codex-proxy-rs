use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use gateway_core::{
    engine::source_admission::{SourceAdmissionError, SourceAdmissionPort, SourceAdmissionRequest},
    identity::{ChannelId, QuotaScopeId},
    lifecycle::CancellationToken,
    policy::RateLimits,
    routing::source::SourceId,
    task::DaemonTask,
};
use gateway_store::{
    StoreResult,
    redis::{
        ClientAdmissionDecision, ClientAdmissionRepository, ClientAdmissionRequest,
        ClientAdmissionRestore, ClientAdmissionRestoreResult, RedisClientAdmissionRepository,
        RedisSourceAdmissionPort,
    },
};
use redis::aio::ConnectionManager;
use tokio::sync::Notify;

fn request(channel: &str, concurrency: u64, rpm: u64, shared: bool) -> SourceAdmissionRequest {
    SourceAdmissionRequest {
        source: SourceId::Channel(ChannelId::new(channel).expect("channel")),
        limits: RateLimits {
            max_concurrency: concurrency,
            requests_per_minute: rpm,
        },
        shared_quota: shared.then(|| {
            (
                QuotaScopeId::new("quota_shared").expect("quota"),
                RateLimits {
                    max_concurrency: 1,
                    requests_per_minute: 0,
                },
            )
        }),
        deadline: SystemTime::now() + Duration::from_secs(30),
    }
}

async fn repository() -> Option<(RedisClientAdmissionRepository, ConnectionManager, String)> {
    let url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(url).expect("Redis URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("Redis connection");
    let namespace = format!("source-admission-test-{}", uuid::Uuid::new_v4());
    let repository =
        RedisClientAdmissionRepository::new(connection.clone(), &namespace).expect("repository");
    Some((repository, connection, namespace))
}

async fn cleanup(connection: &mut ConnectionManager, namespace: &str) {
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(connection)
        .await
        .expect("keys");
    if !keys.is_empty() {
        redis::cmd("DEL")
            .arg(keys)
            .query_async::<u64>(connection)
            .await
            .expect("cleanup");
    }
}

async fn eventually_acquire(
    port: &RedisSourceAdmissionPort,
    request: SourceAdmissionRequest,
) -> Box<dyn gateway_core::engine::provider::ResourceLease> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match port.acquire(request.clone()).await {
                Ok(lease) => break lease,
                Err(SourceAdmissionError::Capacity) => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Err(error) => panic!("admission infrastructure: {error}"),
            }
        }
    })
    .await
    .expect("release converges")
}

#[tokio::test]
async fn independent_channels_share_atomic_quota_and_rejected_attempts_consume_nothing() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let (port, worker) = RedisSourceAdmissionPort::new(Arc::new(repository));
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let running = tokio::spawn(async move { worker.run(stop).await });
    let a = port
        .acquire(request("chan_a", 1, 0, true))
        .await
        .expect("A lease");
    assert!(matches!(
        port.acquire(request("chan_b", 1, 1, true)).await,
        Err(SourceAdmissionError::Capacity)
    ));
    let independent = port
        .acquire(request("chan_c", 1, 0, false))
        .await
        .expect("unrelated channel");
    drop(a);
    let b = eventually_acquire(&port, request("chan_b", 1, 1, true)).await;
    assert!(matches!(
        port.acquire(request("chan_a", 1, 0, true)).await,
        Err(SourceAdmissionError::Capacity)
    ));
    drop(b);
    let a = eventually_acquire(&port, request("chan_a", 1, 0, true)).await;
    drop(a);
    // 并发释放后，B 已接受的 RPM 仍然保留。
    assert!(matches!(
        port.acquire(request("chan_b", 0, 1, false)).await,
        Err(SourceAdmissionError::Capacity)
    ));
    drop(independent);
    cancellation.cancel();
    running.await.expect("worker").expect("shutdown");
    cleanup(&mut connection, &namespace).await;
}

#[tokio::test]
async fn simultaneous_attempts_share_source_capacity_and_lowered_limits_count_existing_work() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let (port, worker) = RedisSourceAdmissionPort::new(Arc::new(repository));
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let running = tokio::spawn(async move { worker.run(stop).await });
    let outcomes =
        futures::future::join_all((0..20).map(|_| port.acquire(request("chan_busy", 2, 0, false))))
            .await;
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 2);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(SourceAdmissionError::Capacity)))
            .count(),
        18
    );
    let old = port
        .acquire(request("chan_unlimited", 0, 0, false))
        .await
        .expect("unlimited source");
    assert!(matches!(
        port.acquire(request("chan_unlimited", 1, 0, false)).await,
        Err(SourceAdmissionError::Capacity)
    ));
    drop(old);
    let next = eventually_acquire(&port, request("chan_unlimited", 1, 0, false)).await;
    drop(next);
    drop(outcomes);
    cancellation.cancel();
    running.await.expect("worker").expect("shutdown");
    cleanup(&mut connection, &namespace).await;
}

#[derive(Default)]
struct DelayedRepository {
    entered: Notify,
    finish: Notify,
    released: Notify,
    admissions: Mutex<Vec<ClientAdmissionRequest>>,
    releases: Mutex<Vec<(Vec<String>, String)>>,
}

#[async_trait]
impl ClientAdmissionRepository for DelayedRepository {
    async fn admit_client_request(
        &self,
        request: &ClientAdmissionRequest,
    ) -> StoreResult<ClientAdmissionDecision> {
        self.admissions
            .lock()
            .expect("admissions")
            .push(request.clone());
        self.entered.notify_one();
        self.finish.notified().await;
        Ok(ClientAdmissionDecision::Granted)
    }
    async fn release_client_request(&self, scopes: &[String], id: &str) -> StoreResult<bool> {
        self.releases
            .lock()
            .expect("releases")
            .push((scopes.to_vec(), id.to_owned()));
        self.released.notify_one();
        Ok(true)
    }
    async fn restore_client_admission(
        &self,
        _: &ClientAdmissionRestore,
    ) -> StoreResult<ClientAdmissionRestoreResult> {
        panic!("source leases do not fabricate logical request recovery")
    }
    async fn clear_client_admission(&self, _: &str) -> StoreResult<()> {
        panic!("source admission never clears a whole scope")
    }
}

#[tokio::test]
async fn cancelled_waiter_releases_late_grant_and_each_attempt_uses_unique_lease_identity() {
    let repository = Arc::new(DelayedRepository::default());
    let (port, worker) = RedisSourceAdmissionPort::new(repository.clone());
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let running = tokio::spawn(async move { worker.run(stop).await });
    for _ in 0..2 {
        let caller = port.clone();
        let waiter =
            tokio::spawn(async move { caller.acquire(request("chan_same", 1, 0, true)).await });
        tokio::time::timeout(Duration::from_secs(2), repository.entered.notified())
            .await
            .expect("entered Redis");
        waiter.abort();
        assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
        assert_eq!(
            repository.releases.lock().expect("releases").len(),
            repository.admissions.lock().expect("admissions").len() - 1,
            "release must not overtake in-flight grant"
        );
        repository.finish.notify_one();
        tokio::time::timeout(Duration::from_secs(2), repository.released.notified())
            .await
            .expect("late grant released");
    }
    let admissions = repository.admissions.lock().expect("admissions").clone();
    assert_ne!(
        admissions[0].model_request_id,
        admissions[1].model_request_id
    );
    let releases = repository.releases.lock().expect("releases").clone();
    for (admission, (scopes, id)) in admissions.iter().zip(releases) {
        assert_eq!(id, admission.model_request_id);
        assert_eq!(
            scopes,
            vec!["source:channel:chan_same", "quota:quota_shared"]
        );
    }
    cancellation.cancel();
    running.await.expect("worker").expect("shutdown");
}

#[tokio::test]
async fn expired_request_and_closed_worker_fail_without_contacting_repository() {
    let repository = Arc::new(DelayedRepository::default());
    let (port, worker) = RedisSourceAdmissionPort::new(repository.clone());
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let running = tokio::spawn(async move { worker.run(stop).await });
    let mut expired = request("chan_expired", 1, 0, false);
    expired.deadline = SystemTime::now() - Duration::from_secs(1);
    assert!(matches!(
        port.acquire(expired).await,
        Err(SourceAdmissionError::Unavailable)
    ));
    assert!(repository.admissions.lock().expect("admissions").is_empty());
    cancellation.cancel();
    running.await.expect("worker").expect("shutdown");
    assert!(matches!(
        port.acquire(request("chan_closed", 1, 0, false)).await,
        Err(SourceAdmissionError::Unavailable)
    ));
}
