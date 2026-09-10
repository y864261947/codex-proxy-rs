use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_store::postgres::{
    ClientApiKeySnapshot, PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

use super::TestDatabase;

#[tokio::test]
async fn channel_snapshot_freezes_public_policy_and_revision_without_reading_credentials() {
    let Some(database) = TestDatabase::create("channel_snapshot").await else {
        return;
    };
    sqlx::query("insert into upstream_channels (id,provider_kind,name,priority,weight,max_concurrency,requests_per_minute,provider_config_json) values ('chan_snapshot','openai_api','A channel',2,3,4,60,'{\"private\":\"sensitive-test-token\"}'::jsonb)").execute(&database.pool).await.expect("channel");
    let repository = PgRuntimeSnapshotRepository::new(database.pool.clone());
    let snapshot = repository.load_runtime_snapshot().await.expect("snapshot");
    let channel = &snapshot.channels[0];
    assert_eq!(channel.binding().id().as_str(), "chan_snapshot");
    assert_eq!(channel.binding().revision().get(), 1);
    assert_eq!(channel.policy().effective_preference(None).priority(), 2);
    assert_eq!(channel.policy().limits().max_concurrency, 4);
    assert!(channel.policy().enabled());
    assert!(!format!("{snapshot:?}").contains("sensitive-test-token"));
    sqlx::query("update upstream_channels set enabled=false,connection_revision=2,max_concurrency=1,name='Renamed' where id='chan_snapshot'").execute(&database.pool).await.expect("change");
    let next = repository
        .load_runtime_snapshot()
        .await
        .expect("next snapshot");
    assert!(!next.channels[0].policy().enabled());
    assert_eq!(next.channels[0].binding().revision().get(), 2);
    assert_eq!(next.channels[0].policy().limits().max_concurrency, 1);
    assert_eq!(channel.policy().snapshot().name(), Some("A channel"));
}

#[test]
fn snapshot_client_policy_contains_only_common_limits() {
    let policy = ClientApiKeySnapshot {
        customer: None,
        access_group: None,
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
         insert into client_api_keys (id, name, key, customer_id, max_concurrency, requests_per_minute, created_at, updated_at)
         values ('key_snapshot', 'first', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'cust_snapshot', 2, 30, now(), now());"
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

#[tokio::test]
async fn snapshot_reads_access_models_pools_and_limits_in_one_consistent_revision() {
    let Some(database) = TestDatabase::create("access_snapshot").await else {
        return;
    };
    sqlx::raw_sql(
        "insert into account_groups (id, name, color, created_at, updated_at) values
         ('grp_00000000000000000000000000000001', 'First', '#123456FF', now(), now()),
         ('grp_00000000000000000000000000000002', 'Second', '#654321FF', now(), now());
         insert into access_groups (id, name, max_concurrency, requests_per_minute, allowed_models)
         values ('access_snapshot', 'Access', 5, 90, array['public-model']);
         insert into access_group_pools values ('access_snapshot', 'grp_00000000000000000000000000000001');
         insert into client_api_keys (id, name, key, access_group_id, max_concurrency, created_at, updated_at)
         values ('key_snapshot', 'Key', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'access_snapshot', 2, now(), now());
         insert into client_api_key_groups (client_api_key_id, account_group_id, created_at)
         values ('key_snapshot', 'grp_00000000000000000000000000000002', now());"
    ).execute(&database.pool).await.expect("seed access group and legacy binding");
    let repository = PgRuntimeSnapshotRepository::new(database.pool.clone());
    let original = repository.load_runtime_snapshot().await.expect("snapshot");
    let key = &original.client_api_keys[0];
    assert_eq!(key.limits.max_concurrency, 2);
    assert_eq!(
        key.group_ids[0].as_str(),
        "grp_00000000000000000000000000000002"
    );
    let group = key.access_group.as_ref().expect("access group");
    assert_eq!(group.limits.max_concurrency, 5);
    assert_eq!(group.limits.requests_per_minute, 90);
    assert!(group.allows_model("public-model"));
    assert_eq!(
        group.pool_group_ids.iter().next().expect("pool").as_str(),
        "grp_00000000000000000000000000000001"
    );
    let error = sqlx::query("delete from access_groups where id = 'access_snapshot'")
        .execute(&database.pool)
        .await
        .expect_err("bound group deletion is restricted");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23001")
    );
    sqlx::raw_sql("update access_groups set enabled = false, allowed_models = '{}' where id = 'access_snapshot'; delete from access_group_pools;")
        .execute(&database.pool).await.expect("change group permissions");
    let current = repository
        .load_runtime_snapshot()
        .await
        .expect("new snapshot");
    let changed = current.client_api_keys[0]
        .access_group
        .as_ref()
        .expect("changed group");
    assert!(!changed.enabled);
    assert!(changed.allowed_models.is_empty());
    assert!(changed.pool_group_ids.is_empty());
    assert!(group.enabled);
    assert!(group.allows_model("public-model"));
    database.close().await;
}
