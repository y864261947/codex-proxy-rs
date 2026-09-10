use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_store::postgres::{
    ClientApiKeySnapshot, PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

use super::TestDatabase;

#[test]
fn snapshot_client_policy_contains_only_common_limits() {
    let policy = ClientApiKeySnapshot {
        customer: None,
        id: ClientApiKeyId::new("key-1").expect("client key ID"),
        plaintext_key: PlaintextClientApiKey::new("sk_snapshot_secret").expect("plaintext key"),
        group_ids: Vec::new(),
        limits: RateLimits {
            max_concurrency: 3,
            requests_per_minute: 60,
        },
    };
    assert_eq!(policy.limits.max_concurrency, 3);
    assert!(policy.group_ids.is_empty());
    assert!(!format!("{policy:?}").contains("sk_snapshot_secret"));
}

#[tokio::test]
async fn runtime_snapshot_loads_enabled_plaintext_key_without_debug_exposure() {
    let Some(database) = TestDatabase::create("client_snapshot").await else {
        return;
    };
    let plaintext = format!("sk_{}", "s".repeat(43));
    sqlx::query(
        "insert into client_api_keys (
           id, name, key, enabled, max_concurrency, requests_per_minute, created_at, updated_at
         ) values ('key_snapshot', 'snapshot', $1, true, 2, 60, now(), now())",
    )
    .bind(&plaintext)
    .execute(&database.pool)
    .await
    .expect("seed client API key");
    let snapshot = PgRuntimeSnapshotRepository::new(database.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("load runtime snapshot");
    assert_eq!(snapshot.client_api_keys.len(), 1);
    assert!(snapshot.client_api_keys[0].group_ids.is_empty());
    assert_eq!(
        snapshot.client_api_keys[0].plaintext_key.expose_for_auth(),
        plaintext
    );
    assert!(!format!("{snapshot:?}").contains(&plaintext));
    database.close().await;
}

#[tokio::test]
async fn snapshot_freezes_customer_status_and_limits_without_changing_key_limits() {
    let Some(database) = TestDatabase::create("customer_snapshot").await else {
        return;
    };
    sqlx::raw_sql(
        "insert into customers (id, name, max_concurrency, requests_per_minute)
         values ('cust_snapshot', 'Customer', 5, 120);
         insert into client_api_keys (id, name, key, customer_id, max_concurrency, requests_per_minute)
         values ('key_snapshot', 'first', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'cust_snapshot', 2, 30);"
    ).execute(&database.pool).await.expect("seed customer and assigned key");
    let repository = PgRuntimeSnapshotRepository::new(database.pool.clone());
    let snapshot = repository
        .load_runtime_snapshot()
        .await
        .expect("customer snapshot");
    let key = &snapshot.client_api_keys[0];
    assert_eq!(key.limits.max_concurrency, 2);
    let customer = key.customer.as_ref().expect("frozen customer");
    assert_eq!(customer.id.as_str(), "cust_snapshot");
    assert!(customer.enabled);
    assert_eq!(
        customer.limits,
        RateLimits {
            max_concurrency: 5,
            requests_per_minute: 120
        }
    );
    sqlx::query(
        "update customers set enabled = false, max_concurrency = 1 where id = 'cust_snapshot'",
    )
    .execute(&database.pool)
    .await
    .expect("disable customer");
    let next = repository
        .load_runtime_snapshot()
        .await
        .expect("updated snapshot");
    let updated = next.client_api_keys[0]
        .customer
        .as_ref()
        .expect("customer still bound");
    assert!(!updated.enabled);
    assert_eq!(updated.limits.max_concurrency, 1);
    assert!(
        customer.enabled,
        "already frozen requests retain their policy"
    );
    database.close().await;
}
