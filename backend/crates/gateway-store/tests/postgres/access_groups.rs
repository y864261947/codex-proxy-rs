use std::collections::BTreeSet;

use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        access_groups::{AccessGroupChange, AccessGroupFields, AccessGroupListQuery},
    },
    ports::store::{AccessGroupStore, AdminStoreErrorKind},
};
use gateway_core::{
    account::scope::AccountGroupId,
    policy::{AccessGroupId, ClientApiKeyId, RateLimits},
};
use gateway_store::postgres::{
    PgAccessGroupRepository, PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

use super::TestDatabase;

fn group() -> AccessGroupId {
    AccessGroupId::new("access_team").expect("group")
}
fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: "req_access_admin".to_owned(),
    }
}
fn fields(pool: &str) -> AccessGroupFields {
    AccessGroupFields {
        name: "100% Team".to_owned(),
        note: Some("private note".to_owned()),
        enabled: true,
        limits: RateLimits {
            max_concurrency: 8,
            requests_per_minute: 120,
        },
        allowed_models: BTreeSet::from(["public-model".to_owned()]),
        pool_group_ids: BTreeSet::from([AccountGroupId::new(pool).expect("pool")]),
    }
}
fn query() -> AccessGroupListQuery {
    AccessGroupListQuery {
        page: 1,
        page_size: PageSize::new(20).expect("page"),
        search: None,
    }
}

#[tokio::test]
async fn access_configuration_assignment_and_restriction_failures_preserve_atomic_state() {
    let Some(db) = TestDatabase::create("access_admin").await else {
        return;
    };
    const POOL: &str = "grp_00000000000000000000000000000001";
    const MISSING_POOL: &str = "grp_00000000000000000000000000000002";
    sqlx::raw_sql(
        "insert into account_groups (id, name, color, created_at, updated_at)
         values ('grp_00000000000000000000000000000001', 'Pool', '#123456', now(), now());
         insert into customers (id, name) values ('cust_team', 'Customer');
         insert into client_api_keys (id, name, key, customer_id, max_concurrency, created_at, updated_at)
         values ('key_access', 'Key', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'cust_team', 2, now(), now());
         insert into client_api_key_groups (client_api_key_id, account_group_id, created_at)
         values ('key_access', 'grp_00000000000000000000000000000001', now());"
    ).execute(&db.pool).await.expect("seed independent customer, key and pool");
    let repository = PgAccessGroupRepository::new(db.pool.clone());
    let created = repository
        .change_access_group(
            AccessGroupChange::Create {
                id: group(),
                fields: fields(POOL),
            },
            &context(),
        )
        .await
        .expect("create");
    let assigned = repository
        .change_access_group(
            AccessGroupChange::AssignKey {
                key_id: ClientApiKeyId::new("key_access").expect("key"),
                access_group_id: Some(group()),
            },
            &context(),
        )
        .await
        .expect("assign");
    assert_eq!(assigned.get(), created.get() + 1);
    let initial = repository.list_access_groups(query()).await.expect("list");
    assert_eq!(initial.items[0].key_count, 1);
    assert_eq!(initial.items[0].fields, fields(POOL));
    let mut search = query();
    search.search = Some("%".to_owned());
    assert_eq!(
        repository
            .list_access_groups(search)
            .await
            .expect("literal search")
            .total,
        1
    );
    for command in [
        AccessGroupChange::Create {
            id: AccessGroupId::new("access_duplicate").expect("group"),
            fields: fields(POOL),
        },
        AccessGroupChange::Update {
            id: group(),
            fields: fields(MISSING_POOL),
        },
        AccessGroupChange::Delete { id: group() },
        AccessGroupChange::AssignKey {
            key_id: ClientApiKeyId::new("key_access").expect("key"),
            access_group_id: Some(AccessGroupId::new("access_missing").expect("group")),
        },
    ] {
        let error = repository
            .change_access_group(command, &context())
            .await
            .expect_err("conflict");
        assert_eq!(error.kind(), AdminStoreErrorKind::Conflict);
        assert_eq!(
            repository
                .list_access_groups(query())
                .await
                .expect("unchanged state"),
            initial
        );
    }
    let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("snapshot");
    let key = &snapshot.client_api_keys[0];
    assert_eq!(key.limits.max_concurrency, 2);
    assert_eq!(
        key.customer.as_ref().expect("customer").id.as_str(),
        "cust_team"
    );
    assert_eq!(key.group_ids[0].as_str(), POOL);
    assert_eq!(
        key.access_group
            .as_ref()
            .expect("group")
            .limits
            .max_concurrency,
        8
    );
    let audit: Vec<Vec<String>> = sqlx::query_scalar(
        "select changed_fields from admin_audit_events order by config_revision",
    )
    .fetch_all(&db.pool)
    .await
    .expect("audit");
    assert_eq!(audit.len(), 2);
    assert!(audit[0].contains(&"allowed_models".to_owned()));
    assert!(audit[0].contains(&"pool_group_ids".to_owned()));
    assert_eq!(audit[1], ["access_group_id"]);
    assert!(!format!("{audit:?}").contains("private note"));
    let mut empty = fields(POOL);
    empty.allowed_models.clear();
    empty.pool_group_ids.clear();
    empty.enabled = false;
    repository
        .change_access_group(
            AccessGroupChange::Update {
                id: group(),
                fields: empty.clone(),
            },
            &context(),
        )
        .await
        .expect("clear permissions");
    assert_eq!(
        repository
            .list_access_groups(query())
            .await
            .expect("empty group")
            .items[0]
            .fields,
        empty
    );
    repository
        .change_access_group(
            AccessGroupChange::AssignKey {
                key_id: ClientApiKeyId::new("key_access").expect("key"),
                access_group_id: None,
            },
            &context(),
        )
        .await
        .expect("explicit unassignment");
    repository
        .change_access_group(AccessGroupChange::Delete { id: group() }, &context())
        .await
        .expect("delete unbound group");
    assert_eq!(
        repository
            .list_access_groups(query())
            .await
            .expect("no groups")
            .total,
        0
    );
    let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("legacy snapshot");
    assert!(snapshot.client_api_keys[0].access_group.is_none());
    assert_eq!(snapshot.client_api_keys[0].group_ids[0].as_str(), POOL);
    db.close().await;
}
