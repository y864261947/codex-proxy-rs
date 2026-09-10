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
        channel_ids: std::collections::BTreeSet::new(),
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
async fn channel_permissions_are_explicit_atomic_and_preserved_in_frozen_key_snapshots() {
    use gateway_core::identity::ChannelId;
    let Some(db) = TestDatabase::create("channel_access").await else {
        return;
    };
    sqlx::raw_sql("insert into upstream_channels (id,provider_kind,name,provider_config_json) values ('chan_selected','openai_api','Selected','{\"key\":\"test-private\"}'::jsonb),('chan_other','openai_api','Other','{\"key\":\"test-other\"}'::jsonb); insert into client_api_keys (id,name,key,created_at,updated_at) values ('key_access','Key','sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ',now(),now());").execute(&db.pool).await.expect("seed");
    let repository = PgAccessGroupRepository::new(db.pool.clone());
    let mut permissions = fields("grp_00000000000000000000000000000001");
    permissions.pool_group_ids.clear();
    permissions
        .channel_ids
        .insert(ChannelId::new("chan_selected").expect("channel"));
    repository
        .change_access_group(
            AccessGroupChange::Create {
                id: group(),
                fields: permissions.clone(),
            },
            &context(),
        )
        .await
        .expect("create channel-only group");
    repository
        .change_access_group(
            AccessGroupChange::AssignKey {
                key_id: ClientApiKeyId::new("key_access").expect("key"),
                access_group_id: Some(group()),
            },
            &context(),
        )
        .await
        .expect("assign");
    let initial = repository.list_access_groups(query()).await.expect("list");
    assert_eq!(initial.items[0].fields, permissions);
    let frozen = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("snapshot");
    let access = frozen.client_api_keys[0]
        .access_group
        .as_ref()
        .expect("access");
    assert!(access.pool_group_ids.is_empty());
    assert_eq!(access.channel_ids, permissions.channel_ids);
    let mut invalid = permissions.clone();
    invalid
        .channel_ids
        .insert(ChannelId::new("chan_missing").expect("channel"));
    assert_eq!(
        repository
            .change_access_group(
                AccessGroupChange::Update {
                    id: group(),
                    fields: invalid
                },
                &context()
            )
            .await
            .expect_err("missing source rolls back")
            .kind(),
        AdminStoreErrorKind::Conflict
    );
    assert_eq!(
        repository
            .list_access_groups(query())
            .await
            .expect("unchanged"),
        initial
    );
    assert!(
        sqlx::query("delete from upstream_channels where id='chan_selected'")
            .execute(&db.pool)
            .await
            .is_err()
    );
    permissions.channel_ids.clear();
    repository
        .change_access_group(
            AccessGroupChange::Update {
                id: group(),
                fields: permissions,
            },
            &context(),
        )
        .await
        .expect("clear sources");
    let next = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("next snapshot");
    assert!(
        next.client_api_keys[0]
            .access_group
            .as_ref()
            .expect("group")
            .channel_ids
            .is_empty()
    );
    assert_eq!(
        access.channel_ids.len(),
        1,
        "old request permissions remain frozen"
    );
    sqlx::query("delete from upstream_channels where id='chan_selected'")
        .execute(&db.pool)
        .await
        .expect("unbound channel may be removed");
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
         values ('grp_00000000000000000000000000000001', 'Pool', '#123456FF', now(), now());
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

#[tokio::test]
async fn key_creation_atomically_applies_customer_and_access_permissions_or_leaves_no_key() {
    use gateway_admin::{
        model::client_keys::{
            ClientKeyListQuery, ClientKeyPageSize, ClientKeySort, ClientKeySortField, NewClientKey,
            SortDirection,
        },
        ports::store::ClientKeyStore,
    };
    use gateway_core::policy::CustomerId;
    use gateway_store::postgres::PgAdminClientKeyStore;
    let Some(db) = TestDatabase::create("scoped_key_creation").await else {
        return;
    };
    sqlx::raw_sql(
        "insert into customers (id, name) values ('cust_team', 'Customer');
         insert into access_groups (id, name, allowed_models) values ('access_team', 'Team', array['public-model']);
         insert into provider_accounts (
           id, provider_kind, name, email, upstream_user_id, upstream_account_id, plan_type,
           authentication_kind, provider_credentials_json, credential_revision, has_refresh_token,
           access_token_expires_at, next_refresh_at, enabled, credential_state, credential_observed_at, created_at, updated_at
         ) values ('acct_global', 'openai', 'Global', null, 'user-global', null, null, 'oauth', '{}'::jsonb, 1,
           false, null, null, true, 'ready', now(), now(), now());"
    ).execute(&db.pool).await.expect("seed ownership and unrelated global provider");
    let keys = PgAdminClientKeyStore::new(db.pool.clone());
    let command = NewClientKey {
        id: ClientApiKeyId::new("key_scoped").expect("key"),
        name: "scoped".to_owned(),
        label: None,
        customer_id: Some(CustomerId::new("cust_team").expect("customer")),
        access_group_id: Some(group()),
        group_ids: Vec::new(),
        limits: RateLimits {
            max_concurrency: 2,
            requests_per_minute: 20,
        },
        plaintext: format!("sk_{}", "s".repeat(43)),
    };
    let mut missing = command.clone();
    missing.access_group_id = Some(AccessGroupId::new("access_missing").expect("group"));
    let before: i64 =
        sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
            .fetch_one(&db.pool)
            .await
            .expect("revision");
    assert_eq!(
        keys.create_client_key(missing, &context())
            .await
            .expect_err("missing group")
            .kind(),
        AdminStoreErrorKind::Conflict
    );
    let after: (i64, i64, i64) = sqlx::query_as("select config_revision, (select count(*) from client_api_keys), (select count(*) from admin_audit_events) from runtime_settings where id = 1")
        .fetch_one(&db.pool).await.expect("no partial create");
    assert_eq!(after, (before, 0, 0));
    let (revision, record) = keys
        .create_client_key(command, &context())
        .await
        .expect("create scoped key");
    assert_eq!(revision.get(), u64::try_from(before + 1).expect("revision"));
    assert_eq!(record.customer.as_ref().expect("customer").name, "Customer");
    assert_eq!(
        record.access_group.as_ref().expect("access group").name,
        "Team"
    );
    assert!(record.groups.is_empty());
    assert!(
        record.provider_kinds.is_empty(),
        "empty access pool must not advertise global providers"
    );
    let page = keys
        .list_client_keys(ClientKeyListQuery {
            cursor: None,
            page_size: ClientKeyPageSize::new(20).expect("page"),
            search: Some("scoped".to_owned()),
            sort: ClientKeySort {
                field: ClientKeySortField::Name,
                direction: SortDirection::Asc,
            },
        })
        .await
        .expect("list scoped key");
    assert_eq!(page.items, [record]);
    db.close().await;
}
