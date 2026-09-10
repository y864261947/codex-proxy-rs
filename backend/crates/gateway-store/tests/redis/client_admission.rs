use std::time::Duration;

use chrono::{DateTime, Utc};
use gateway_store::redis::{
    ClientAdmissionDecision, ClientAdmissionLimits, ClientAdmissionRecentRequest,
    ClientAdmissionRejection, ClientAdmissionRepository, ClientAdmissionRequest,
    ClientAdmissionRestore, ClientAdmissionRestoreResult, ClientAdmissionRunningRequest,
    ClientAdmissionScope, RedisClientAdmissionRepository,
};
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[test]
fn client_admission_rejects_zero_ttl() {
    let request = admission_request("request-1", "key-1", Duration::ZERO);
    assert!(request.validate().is_err());
    let request = admission_request("request-1", "key-1", Duration::from_nanos(1));
    assert!(request.validate().is_err());
}

#[test]
fn admission_rejects_empty_and_duplicate_scopes() {
    let mut request = admission_request("request-1", "key-1", Duration::from_secs(30));
    request.scopes.push(request.scopes[0].clone());
    assert!(request.validate().is_err());
    request.scopes.clear();
    assert!(request.validate().is_err());
}

#[tokio::test]
async fn customer_capacity_is_atomic_across_concurrent_keys_and_released_together() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let requests = (0..20)
        .map(|index| {
            let mut request = admission_request(
                &format!("request-{index}"),
                &format!("key-{index}"),
                Duration::from_secs(30),
            );
            request.scopes.push(ClientAdmissionScope {
                scope_ref: "customer:shared".to_owned(),
                limits: ClientAdmissionLimits {
                    max_concurrency: 3,
                    requests_per_minute: 0,
                },
            });
            request
        })
        .collect::<Vec<_>>();
    let results = futures::future::join_all(
        requests
            .iter()
            .map(|request| repository.admit_client_request(request)),
    )
    .await;
    let granted = results
        .iter()
        .enumerate()
        .filter_map(|(index, decision)| {
            let decision = decision.as_ref().expect("atomic admission");
            if *decision == ClientAdmissionDecision::Granted {
                Some(index)
            } else {
                assert_eq!(
                    *decision,
                    ClientAdmissionDecision::Rejected(ClientAdmissionRejection::ConcurrencyLimited)
                );
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(granted.len(), 3);
    let winner = &requests[granted[0]];
    assert_eq!(
        repository
            .admit_client_request(winner)
            .await
            .expect("idempotent admission at capacity"),
        ClientAdmissionDecision::Granted
    );
    let scopes = winner
        .scopes
        .iter()
        .map(|scope| scope.scope_ref.clone())
        .collect::<Vec<_>>();
    assert!(
        repository
            .release_client_request(&scopes, &winner.model_request_id)
            .await
            .expect("release all scopes")
    );
    assert!(
        !repository
            .release_client_request(&scopes, &winner.model_request_id)
            .await
            .expect("idempotent release")
    );
    let rejected = (0..requests.len())
        .find(|index| !granted.contains(index))
        .expect("rejected request");
    assert_eq!(
        repository
            .admit_client_request(&requests[rejected])
            .await
            .expect("reuse parent slot"),
        ClientAdmissionDecision::Granted
    );
    let mut child_only = winner.clone();
    child_only.model_request_id = "request-child-after-release".to_owned();
    child_only.scopes.truncate(1);
    child_only.scopes[0].limits.max_concurrency = 1;
    assert_eq!(
        repository
            .admit_client_request(&child_only)
            .await
            .expect("child slot also released"),
        ClientAdmissionDecision::Granted
    );
    delete_namespace_keys(&mut connection, &namespace).await;
}

#[tokio::test]
async fn rejected_parent_does_not_consume_child_rpm_and_release_preserves_parent_rpm() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let parent = ClientAdmissionScope {
        scope_ref: "customer:rpm".to_owned(),
        limits: ClientAdmissionLimits {
            max_concurrency: 0,
            requests_per_minute: 1,
        },
    };
    let mut first = admission_request("first", "key-a", Duration::from_secs(30));
    first.scopes.push(parent.clone());
    assert_eq!(
        repository
            .admit_client_request(&first)
            .await
            .expect("first admission"),
        ClientAdmissionDecision::Granted
    );
    let refs = first
        .scopes
        .iter()
        .map(|scope| scope.scope_ref.clone())
        .collect::<Vec<_>>();
    repository
        .release_client_request(&refs, "first")
        .await
        .expect("release first");
    let mut second = admission_request("second", "key-b", Duration::from_secs(30));
    second.scopes[0].limits.requests_per_minute = 1;
    second.scopes.push(parent);
    assert_eq!(
        repository
            .admit_client_request(&second)
            .await
            .expect("shared RPM after release"),
        ClientAdmissionDecision::Rejected(ClientAdmissionRejection::RateLimited)
    );
    second.model_request_id = "third".to_owned();
    second.scopes.truncate(1);
    assert_eq!(
        repository
            .admit_client_request(&second)
            .await
            .expect("rejected request consumed no child quota"),
        ClientAdmissionDecision::Granted
    );
    delete_namespace_keys(&mut connection, &namespace).await;
}

#[tokio::test]
async fn rejected_child_does_not_reserve_an_earlier_parent_scope() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let mut occupied = admission_request("occupied", "key-child", Duration::from_secs(30));
    occupied.scopes[0].limits.max_concurrency = 1;
    assert_eq!(
        repository
            .admit_client_request(&occupied)
            .await
            .expect("occupy child"),
        ClientAdmissionDecision::Granted
    );
    let parent = ClientAdmissionScope {
        scope_ref: "customer:ordered".to_owned(),
        limits: ClientAdmissionLimits {
            max_concurrency: 1,
            requests_per_minute: 1,
        },
    };
    let mut rejected = occupied.clone();
    rejected.model_request_id = "rejected".to_owned();
    rejected.scopes.insert(0, parent.clone());
    assert_eq!(
        repository
            .admit_client_request(&rejected)
            .await
            .expect("child full"),
        ClientAdmissionDecision::Rejected(ClientAdmissionRejection::ConcurrencyLimited)
    );
    let mut next = admission_request("next", "key-other", Duration::from_secs(30));
    next.scopes.insert(0, parent);
    assert_eq!(
        repository
            .admit_client_request(&next)
            .await
            .expect("parent remains free"),
        ClientAdmissionDecision::Granted
    );
    delete_namespace_keys(&mut connection, &namespace).await;
}

#[test]
fn client_admission_rejects_values_outside_redis_exact_integer_range() {
    let mut request = admission_request("request-1", "key-1", Duration::from_secs(30));
    request.scopes[0].limits.max_concurrency = 1_u64 << 53;
    assert!(request.validate().is_err());
}

#[test]
fn client_admission_restore_rejects_duplicate_request_ids() {
    let started_at = Utc::now();
    let recovery = ClientAdmissionRestore {
        scope_ref: "key-1".to_owned(),
        recent_requests: vec![
            recent_request("request-1", started_at),
            recent_request("request-1", started_at),
        ],
        running_requests: Vec::new(),
    };
    assert!(recovery.validate().is_err());
}

#[tokio::test]
async fn restore_rebuilds_lost_cache_without_overwriting_new_admission() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let key_ref = "key-cache-recovery";
    let old = admission_request("request-before-crash", key_ref, Duration::from_secs(30));
    assert_eq!(
        repository
            .admit_client_request(&old)
            .await
            .expect("admit request before cache loss"),
        ClientAdmissionDecision::Granted
    );
    delete_namespace_keys(&mut connection, &namespace).await;

    let live = admission_request("request-after-crash", key_ref, Duration::from_secs(30));
    assert_eq!(
        repository
            .admit_client_request(&live)
            .await
            .expect("admit concurrent request after cache loss"),
        ClientAdmissionDecision::Granted
    );
    let redis_now = redis_now(&mut connection).await;
    let recovery = ClientAdmissionRestore {
        scope_ref: key_ref.to_owned(),
        recent_requests: vec![
            recent_request(
                "request-before-crash",
                redis_now - chrono::Duration::seconds(2),
            ),
            recent_request(
                "request-after-crash",
                redis_now - chrono::Duration::seconds(1),
            ),
        ],
        running_requests: vec![
            running_request(
                "request-before-crash",
                redis_now + chrono::Duration::seconds(180),
            ),
            running_request(
                "request-after-crash",
                redis_now + chrono::Duration::seconds(30),
            ),
        ],
    };

    let restored = repository
        .restore_client_admission(&recovery)
        .await
        .expect("merge durable facts into live admission state");
    assert_eq!(
        restored,
        ClientAdmissionRestoreResult {
            restored_recent_requests: 1,
            restored_running_requests: 1,
        }
    );
    assert_eq!(
        repository
            .restore_client_admission(&recovery)
            .await
            .expect("repeat idempotent recovery"),
        ClientAdmissionRestoreResult {
            restored_recent_requests: 0,
            restored_running_requests: 0,
        }
    );

    let keys = namespace_keys(&mut connection, &namespace).await;
    assert_eq!(
        zcard(&mut connection, key_with_suffix(&keys, ":active")).await,
        2
    );
    assert_eq!(
        zcard(&mut connection, key_with_suffix(&keys, ":requests")).await,
        2
    );
    let active_ttl = pttl(&mut connection, key_with_suffix(&keys, ":active")).await;
    assert!((230_000..=245_000).contains(&active_ttl));

    let mut probe = admission_request("request-probe", key_ref, Duration::from_secs(30));
    probe.scopes[0].limits.requests_per_minute = 3;
    assert_eq!(
        repository
            .admit_client_request(&probe)
            .await
            .expect("enforce restored concurrency"),
        ClientAdmissionDecision::Rejected(ClientAdmissionRejection::ConcurrencyLimited)
    );
    assert!(
        repository
            .release_client_request(&[key_ref.to_owned()], "request-before-crash")
            .await
            .expect("release restored request by durable ID")
    );
    assert_eq!(
        repository
            .admit_client_request(&probe)
            .await
            .expect("reuse released restored slot within RPM limit"),
        ClientAdmissionDecision::Granted
    );
    assert!(
        repository
            .release_client_request(&[key_ref.to_owned()], "request-after-crash")
            .await
            .expect("release admission created during recovery")
    );
    let mut rate_probe = admission_request("request-rate-probe", key_ref, Duration::from_secs(30));
    rate_probe.scopes[0].limits.requests_per_minute = 3;
    assert_eq!(
        repository
            .admit_client_request(&rate_probe)
            .await
            .expect("enforce restored RPM without duplicate request facts"),
        ClientAdmissionDecision::Rejected(ClientAdmissionRejection::RateLimited)
    );

    repository
        .clear_client_admission(key_ref)
        .await
        .expect("clean client admission state");
}

#[tokio::test]
async fn restore_uses_redis_time_for_window_and_running_expiry_boundaries() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let key_ref = "key-time-boundary";
    let redis_now = redis_now(&mut connection).await;
    let recovery = ClientAdmissionRestore {
        scope_ref: key_ref.to_owned(),
        recent_requests: vec![
            recent_request(
                "request-at-cutoff",
                redis_now - chrono::Duration::seconds(60),
            ),
            recent_request(
                "request-inside-window",
                redis_now - chrono::Duration::seconds(59),
            ),
        ],
        running_requests: vec![
            running_request(
                "request-expired",
                redis_now - chrono::Duration::milliseconds(1),
            ),
            running_request("request-live", redis_now + chrono::Duration::seconds(180)),
        ],
    };

    assert_eq!(
        repository
            .restore_client_admission(&recovery)
            .await
            .expect("restore time-boundary facts"),
        ClientAdmissionRestoreResult {
            restored_recent_requests: 1,
            restored_running_requests: 1,
        }
    );
    let keys = namespace_keys(&mut connection, &namespace).await;
    let request_members = zmembers(&mut connection, key_with_suffix(&keys, ":requests")).await;
    assert_eq!(request_members, vec!["request-inside-window"]);
    let active_members = zmembers(&mut connection, key_with_suffix(&keys, ":active")).await;
    assert_eq!(active_members, vec!["request-live"]);
    let active_ttl = pttl(&mut connection, key_with_suffix(&keys, ":active")).await;
    assert!((230_000..=245_000).contains(&active_ttl));
    assert!(
        repository
            .release_client_request(&[key_ref.to_owned()], "request-live")
            .await
            .expect("release live recovered request")
    );

    repository
        .clear_client_admission(key_ref)
        .await
        .expect("clean time-boundary state");
}

#[tokio::test]
async fn restore_rejects_future_window_fact_without_partial_write() {
    let Some((repository, mut connection, namespace)) = repository().await else {
        return;
    };
    let redis_now = redis_now(&mut connection).await;
    let recovery = ClientAdmissionRestore {
        scope_ref: "key-future-fact".to_owned(),
        recent_requests: vec![
            recent_request("request-valid", redis_now - chrono::Duration::seconds(1)),
            recent_request("request-future", redis_now + chrono::Duration::seconds(10)),
        ],
        running_requests: Vec::new(),
    };

    assert!(
        repository
            .restore_client_admission(&recovery)
            .await
            .is_err()
    );
    assert!(namespace_keys(&mut connection, &namespace).await.is_empty());
}

fn admission_request(
    model_request_id: &str,
    scope_ref: &str,
    lease_ttl: Duration,
) -> ClientAdmissionRequest {
    ClientAdmissionRequest {
        model_request_id: model_request_id.to_owned(),
        scopes: vec![ClientAdmissionScope {
            scope_ref: scope_ref.to_owned(),
            limits: ClientAdmissionLimits {
                max_concurrency: 2,
                requests_per_minute: 0,
            },
        }],
        lease_ttl,
    }
}

fn recent_request(
    model_request_id: &str,
    started_at: DateTime<Utc>,
) -> ClientAdmissionRecentRequest {
    ClientAdmissionRecentRequest {
        model_request_id: model_request_id.to_owned(),
        started_at,
    }
}

fn running_request(
    model_request_id: &str,
    expires_at: DateTime<Utc>,
) -> ClientAdmissionRunningRequest {
    ClientAdmissionRunningRequest {
        model_request_id: model_request_id.to_owned(),
        expires_at,
    }
}

async fn repository() -> Option<(RedisClientAdmissionRepository, ConnectionManager, String)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(redis_url).expect("valid CPR_TEST_REDIS_URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("connect test Redis");
    let namespace = format!("gateway-store-admission-test-{}", Uuid::new_v4());
    let repository = RedisClientAdmissionRepository::new(connection.clone(), &namespace)
        .expect("valid test namespace");
    Some((repository, connection, namespace))
}

async fn redis_now(connection: &mut ConnectionManager) -> DateTime<Utc> {
    let (seconds, microseconds) = redis::cmd("TIME")
        .query_async::<(i64, i64)>(connection)
        .await
        .expect("read Redis server time");
    DateTime::from_timestamp(
        seconds,
        u32::try_from(microseconds).expect("valid microseconds") * 1_000,
    )
    .expect("Redis time is representable")
}

async fn namespace_keys(connection: &mut ConnectionManager, namespace: &str) -> Vec<String> {
    redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(connection)
        .await
        .expect("list isolated admission keys")
}

async fn delete_namespace_keys(connection: &mut ConnectionManager, namespace: &str) {
    let keys = namespace_keys(connection, namespace).await;
    if !keys.is_empty() {
        redis::cmd("DEL")
            .arg(keys)
            .query_async::<i64>(connection)
            .await
            .expect("delete isolated admission keys");
    }
}

fn key_with_suffix<'a>(keys: &'a [String], suffix: &str) -> &'a str {
    keys.iter()
        .find(|key| key.ends_with(suffix))
        .expect("admission key with expected suffix")
}

async fn zcard(connection: &mut ConnectionManager, key: &str) -> u64 {
    redis::cmd("ZCARD")
        .arg(key)
        .query_async(connection)
        .await
        .expect("read admission cardinality")
}

async fn zmembers(connection: &mut ConnectionManager, key: &str) -> Vec<String> {
    redis::cmd("ZRANGE")
        .arg(key)
        .arg(0)
        .arg(-1)
        .query_async(connection)
        .await
        .expect("read admission members")
}

async fn pttl(connection: &mut ConnectionManager, key: &str) -> i64 {
    redis::cmd("PTTL")
        .arg(key)
        .query_async(connection)
        .await
        .expect("read admission TTL")
}
