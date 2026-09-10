use std::{num::NonZeroU32, time::Duration};

use gateway_core::{
    engine::execution::{ProviderCircuitPolicy, provider_failure_affects_circuit},
    error::ProviderErrorKind,
};
use gateway_store::redis::{
    ProviderCircuitDecision, ProviderCircuitRepository, RedisProviderCircuitRepository,
};
use redis::aio::ConnectionManager;
use uuid::Uuid;

#[test]
fn provider_circuit_default_has_positive_threshold() {
    assert!(ProviderCircuitPolicy::default().failure_threshold.get() > 0);
}

#[test]
fn provider_circuit_should_only_count_instance_attributable_failures() {
    for error_kind in [
        ProviderErrorKind::Timeout,
        ProviderErrorKind::Transport,
        ProviderErrorKind::Protocol,
        ProviderErrorKind::Unavailable,
    ] {
        assert!(provider_failure_affects_circuit(error_kind));
    }
    for error_kind in [
        ProviderErrorKind::InvalidRequest,
        ProviderErrorKind::Unsupported,
        ProviderErrorKind::Unauthorized,
        ProviderErrorKind::PermissionDenied,
        ProviderErrorKind::RateLimited,
        ProviderErrorKind::QuotaExhausted,
        ProviderErrorKind::Cancelled,
        ProviderErrorKind::ProcessTerminated,
        ProviderErrorKind::SourceCapacityUnavailable,
        ProviderErrorKind::AccountCapacityUnavailable,
        ProviderErrorKind::ProviderInfrastructureUnavailable,
    ] {
        assert!(!provider_failure_affects_circuit(error_kind));
    }
}

#[tokio::test]
async fn source_circuits_keep_channels_pools_and_legacy_providers_independent() {
    use gateway_core::{
        engine::execution::{
            ProviderCircuitDecision as Decision, ProviderCircuitPort, ProviderCircuitScope,
        },
        identity::{ChannelId, ProviderKind},
        routing::{AccountGroupId, source::SourceId},
    };
    let Some((repository, mut connection, namespace)) = repository(2).await else {
        return;
    };
    let provider = ProviderKind::new("openai").expect("provider");
    assert!(
        ProviderKind::new("__source:channel:chan_a").is_err(),
        "reserved scope cannot alias a provider"
    );
    let legacy = ProviderCircuitScope::Provider(provider);
    let a = ProviderCircuitScope::Source(SourceId::Channel(ChannelId::new("chan_a").expect("A")));
    let b = ProviderCircuitScope::Source(SourceId::Channel(ChannelId::new("chan_b").expect("B")));
    let pool = ProviderCircuitScope::Source(SourceId::AccountPool(
        AccountGroupId::new("grp_00000000000000000000000000000001").expect("pool"),
    ));
    for scope in [&legacy, &a, &pool] {
        repository
            .observe_failure(scope)
            .await
            .expect("first failure");
        assert_eq!(
            repository.decision(scope).await.expect("below threshold"),
            Decision::Allow
        );
        repository
            .observe_failure(scope)
            .await
            .expect("second failure");
        assert!(matches!(
            repository.decision(scope).await.expect("blocked"),
            Decision::BlockedUntil(_)
        ));
        assert_eq!(
            repository.decision(&b).await.expect("unrelated B"),
            Decision::Allow
        );
    }
    assert!(matches!(
        repository
            .provider_circuit_decision("openai")
            .await
            .expect("legacy key preserved"),
        ProviderCircuitDecision::BlockedUntil(_)
    ));
    repository.observe_success(&b).await.expect("B success");
    repository.observe_success(&a).await.expect("A recovered");
    assert_eq!(
        repository.decision(&a).await.expect("A ready"),
        Decision::Allow
    );
    for scope in [&legacy, &pool] {
        assert!(matches!(
            repository.decision(scope).await.expect("still blocked"),
            Decision::BlockedUntil(_)
        ));
    }
    delete_namespace_keys(&mut connection, &namespace).await;
}

#[tokio::test]
async fn provider_circuit_opens_at_threshold_and_success_resets_it() {
    let Some((repository, mut connection, namespace)) = repository(2).await else {
        return;
    };
    let instance = "instance-circuit-primary";

    assert_eq!(
        repository
            .provider_circuit_decision(instance)
            .await
            .expect("read empty provider circuit"),
        ProviderCircuitDecision::Allow
    );
    let first = repository
        .observe_provider_failure(instance)
        .await
        .expect("record first provider failure");
    assert_eq!(first.failure_count, 1);
    assert!(first.open_until.is_none());

    let second = repository
        .observe_provider_failure(instance)
        .await
        .expect("record threshold provider failure");
    assert_eq!(second.failure_count, 2);
    assert!(second.open_until.is_some());
    assert!(matches!(
        repository
            .provider_circuit_decision(instance)
            .await
            .expect("read open provider circuit"),
        ProviderCircuitDecision::BlockedUntil(_)
    ));

    repository
        .observe_provider_success(instance)
        .await
        .expect("reset provider circuit after success");
    assert_eq!(
        repository
            .provider_circuit_decision(instance)
            .await
            .expect("read reset provider circuit"),
        ProviderCircuitDecision::Allow
    );

    delete_namespace_keys(&mut connection, &namespace).await;
}

#[tokio::test]
async fn provider_circuit_keeps_instances_isolated() {
    let Some((repository, mut connection, namespace)) = repository(1).await else {
        return;
    };

    repository
        .observe_provider_failure("instance-circuit-failed")
        .await
        .expect("open failed instance circuit");
    assert!(matches!(
        repository
            .provider_circuit_decision("instance-circuit-failed")
            .await
            .expect("read failed instance circuit"),
        ProviderCircuitDecision::BlockedUntil(_)
    ));
    assert_eq!(
        repository
            .provider_circuit_decision("instance-circuit-healthy")
            .await
            .expect("read isolated healthy circuit"),
        ProviderCircuitDecision::Allow
    );

    delete_namespace_keys(&mut connection, &namespace).await;
}

#[tokio::test]
async fn provider_circuit_reopens_after_redis_time_deadline() {
    let Some((repository, mut connection, namespace)) =
        repository_with_policy(1, Duration::from_millis(40)).await
    else {
        return;
    };
    let instance = "instance-circuit-deadline";

    repository
        .observe_provider_failure(instance)
        .await
        .expect("open short provider circuit");
    assert!(matches!(
        repository
            .provider_circuit_decision(instance)
            .await
            .expect("read short open circuit"),
        ProviderCircuitDecision::BlockedUntil(_)
    ));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        repository
            .provider_circuit_decision(instance)
            .await
            .expect("read circuit after Redis deadline"),
        ProviderCircuitDecision::Allow
    );

    delete_namespace_keys(&mut connection, &namespace).await;
}

async fn repository(
    threshold: u32,
) -> Option<(RedisProviderCircuitRepository, ConnectionManager, String)> {
    repository_with_policy(threshold, Duration::from_secs(30)).await
}

async fn repository_with_policy(
    threshold: u32,
    open_duration: Duration,
) -> Option<(RedisProviderCircuitRepository, ConnectionManager, String)> {
    let redis_url = crate::support::test_env("CPR_TEST_REDIS_URL")?;
    let client = redis::Client::open(redis_url).expect("valid CPR_TEST_REDIS_URL");
    let connection = client
        .get_connection_manager()
        .await
        .expect("connect test Redis");
    let namespace = format!("gateway-store-circuit-test-{}", Uuid::new_v4());
    let policy = ProviderCircuitPolicy {
        failure_threshold: NonZeroU32::new(threshold).expect("positive threshold"),
        open_duration,
    };
    let repository = RedisProviderCircuitRepository::new(connection.clone(), &namespace, policy)
        .expect("valid provider circuit repository");
    Some((repository, connection, namespace))
}

async fn delete_namespace_keys(connection: &mut ConnectionManager, namespace: &str) {
    let keys = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async::<Vec<String>>(connection)
        .await
        .expect("list isolated provider circuit keys");
    if !keys.is_empty() {
        redis::cmd("DEL")
            .arg(keys)
            .query_async::<i64>(connection)
            .await
            .expect("delete isolated provider circuit keys");
    }
}
