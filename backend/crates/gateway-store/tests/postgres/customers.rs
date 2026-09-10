use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        customers::{CustomerChange, CustomerFields, CustomerListQuery},
    },
    ports::store::{AdminStoreErrorKind, CustomerStore},
};
use gateway_core::policy::{ClientApiKeyId, CustomerId, RateLimits};
use gateway_store::postgres::{
    ClientApiKeyRepository, PgClientApiKeyRepository, PgCustomerRepository,
    PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

use super::TestDatabase;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: "req_customer_admin".to_owned(),
    }
}
fn customer(value: &str) -> CustomerId {
    CustomerId::new(value).expect("customer ID")
}
fn fields(name: &str) -> CustomerFields {
    CustomerFields {
        name: name.to_owned(),
        note: Some("customer note must not enter audit".to_owned()),
        enabled: true,
        limits: RateLimits {
            max_concurrency: 5,
            requests_per_minute: 120,
        },
    }
}
fn query() -> CustomerListQuery {
    CustomerListQuery {
        page: 1,
        page_size: PageSize::new(20).expect("page size"),
        search: None,
    }
}

#[tokio::test]
async fn customer_changes_commit_policy_revision_and_safe_audit_together() {
    let Some(db) = TestDatabase::create("customer_admin_mutations").await else {
        return;
    };
    let store = PgCustomerRepository::new(db.pool.clone());
    let created = store
        .change_customer(
            CustomerChange::Create {
                id: customer("cust_one"),
                fields: fields("Customer One"),
            },
            &context(),
        )
        .await
        .expect("create customer");
    sqlx::query("insert into client_api_keys (id, name, key, max_concurrency, requests_per_minute, created_at, updated_at) values ('key_customer', 'key', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 2, 30, now(), now())")
        .execute(&db.pool).await.expect("seed existing key");
    let assigned = store
        .change_customer(
            CustomerChange::AssignKey {
                key_id: ClientApiKeyId::new("key_customer").expect("key ID"),
                customer_id: Some(customer("cust_one")),
            },
            &context(),
        )
        .await
        .expect("assign key");
    assert_eq!(assigned.get(), created.get() + 1);
    let page = store.list_customers(query()).await.expect("customer list");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].key_count, 1);
    assert_eq!(page.config_revision, assigned);
    let key = PgClientApiKeyRepository::new(db.pool.clone())
        .get_client_api_key("key_customer")
        .await
        .expect("key view")
        .expect("key");
    assert_eq!(
        key.customer.expect("key customer view").name,
        "Customer One"
    );
    assert_eq!(key.max_concurrency, 2);
    assert_eq!(key.requests_per_minute, 30);
    assert!(key.groups.is_empty());
    let mut updated = fields("Customer One");
    updated.enabled = false;
    store
        .change_customer(
            CustomerChange::Update {
                id: customer("cust_one"),
                fields: updated,
            },
            &context(),
        )
        .await
        .expect("disable customer");
    let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("updated policy");
    let policy = snapshot.client_api_keys[0]
        .customer
        .as_ref()
        .expect("customer policy");
    assert!(!policy.enabled);
    assert_eq!(policy.limits.max_concurrency, 5);
    let audits: Vec<(String, String, Vec<String>)> = sqlx::query_as("select entity_kind, actor_ref, changed_fields from admin_audit_events order by config_revision").fetch_all(&db.pool).await.expect("audits");
    assert_eq!(audits.len(), 3);
    assert!(
        audits
            .iter()
            .all(|(_, actor, fields)| actor == "admin_api_key"
                && !fields.iter().any(|field| field.contains("customer note")))
    );
    assert_eq!(audits[1].0, "client_api_key");
    assert_eq!(audits[1].2, ["customer_id"]);
    db.close().await;
}

#[tokio::test]
async fn failed_customer_mutations_roll_back_revision_audit_and_key_ownership() {
    let Some(db) = TestDatabase::create("customer_admin_rollback").await else {
        return;
    };
    let store = PgCustomerRepository::new(db.pool.clone());
    store
        .change_customer(
            CustomerChange::Create {
                id: customer("cust_one"),
                fields: fields("Duplicate"),
            },
            &context(),
        )
        .await
        .expect("create customer");
    sqlx::query("insert into client_api_keys (id, name, key, customer_id, created_at, updated_at) values ('key_customer', 'key', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'cust_one', now(), now())")
        .execute(&db.pool).await.expect("seed assigned key");
    let before = store.list_customers(query()).await.expect("initial state");
    for change in [
        CustomerChange::Create {
            id: customer("cust_two"),
            fields: fields("Duplicate"),
        },
        CustomerChange::Delete {
            id: customer("cust_one"),
        },
        CustomerChange::AssignKey {
            key_id: ClientApiKeyId::new("key_customer").expect("key ID"),
            customer_id: Some(customer("cust_missing")),
        },
    ] {
        assert_eq!(
            store
                .change_customer(change, &context())
                .await
                .expect_err("conflict")
                .kind(),
            AdminStoreErrorKind::Conflict
        );
    }
    let after = store
        .list_customers(query())
        .await
        .expect("state after failed mutations");
    assert_eq!(after, before);
    let audit_count: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .expect("audits");
    assert_eq!(audit_count, 1);
    store
        .change_customer(
            CustomerChange::AssignKey {
                key_id: ClientApiKeyId::new("key_customer").expect("key ID"),
                customer_id: None,
            },
            &context(),
        )
        .await
        .expect("explicit unassignment");
    store
        .change_customer(
            CustomerChange::Delete {
                id: customer("cust_one"),
            },
            &context(),
        )
        .await
        .expect("delete unreferenced customer");
    assert_eq!(
        store
            .list_customers(query())
            .await
            .expect("empty list")
            .total,
        0
    );
    db.close().await;
}

#[test]
fn customer_fields_reject_invalid_names_and_unsafe_limits() {
    let mut draft = fields(" customer ");
    assert!(draft.validate().is_err());
    draft.name = "客户".to_owned();
    assert!(draft.validate().is_ok());
    draft.limits.max_concurrency = 1_u64 << 53;
    assert!(draft.validate().is_err());
}
