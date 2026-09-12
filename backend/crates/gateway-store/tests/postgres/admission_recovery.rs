use chrono::{DateTime, Duration, Utc};
use gateway_core::policy::{AdmissionScopeId, ClientApiKeyId, CustomerId};
use gateway_store::postgres::{
    ClientAdmissionRecentRequest, ClientAdmissionRecovery, ClientAdmissionRecoveryRepository,
    ClientAdmissionRunningRequest, PgClientAdmissionRecoveryRepository,
};
use sqlx::PgPool;

use super::TestDatabase;

#[tokio::test]
async fn recovery_loads_precise_window_and_running_request_facts() {
    let Some(database) = TestDatabase::create("admission_recovery").await else {
        return;
    };
    let now = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .expect("current time is representable at PostgreSQL precision");
    let window_started_at = now - Duration::seconds(60);
    seed_request(
        &database.pool,
        "old-running",
        now - Duration::seconds(120),
        now + Duration::seconds(30),
        "running",
    )
    .await;
    seed_request(
        &database.pool,
        "old-complete",
        now - Duration::seconds(90),
        now - Duration::seconds(30),
        "succeeded",
    )
    .await;
    seed_request(
        &database.pool,
        "recent-complete",
        now - Duration::seconds(20),
        now + Duration::seconds(10),
        "succeeded",
    )
    .await;
    seed_request(
        &database.pool,
        "recent-running",
        now - Duration::seconds(10),
        now + Duration::seconds(40),
        "running",
    )
    .await;

    seed_request(
        &database.pool,
        "internal-probe",
        now - Duration::seconds(5),
        now + Duration::seconds(30),
        "running",
    )
    .await;
    sqlx::query("update model_requests set client_transport = 'internal', client_api_key_ref = 'internal_probe' where id = 'internal-probe'")
        .execute(&database.pool).await.expect("internal probe is not customer admission");
    let repository = PgClientAdmissionRecoveryRepository::new(database.pool.clone());
    let actual = repository
        .load_client_admission_recovery(window_started_at)
        .await
        .expect("load precise admission recovery facts");
    let mut expected = vec![ClientAdmissionRecovery {
        scope_id: AdmissionScopeId::Key(ClientApiKeyId::new("key-recovery").expect("key ID")),
        recent_requests: vec![
            ClientAdmissionRecentRequest {
                model_request_id: "recent-complete".to_owned(),
                started_at: now - Duration::seconds(20),
            },
            ClientAdmissionRecentRequest {
                model_request_id: "recent-running".to_owned(),
                started_at: now - Duration::seconds(10),
            },
        ],
        running_requests: vec![
            ClientAdmissionRunningRequest {
                model_request_id: "old-running".to_owned(),
                deadline_at: now + Duration::seconds(30),
            },
            ClientAdmissionRunningRequest {
                model_request_id: "recent-running".to_owned(),
                deadline_at: now + Duration::seconds(40),
            },
        ],
    }];
    let mut global = expected[0].clone();
    global.scope_id = AdmissionScopeId::Global;
    expected.push(global);
    assert_eq!(actual, expected);

    database.close().await;
}

async fn seed_request(
    pool: &PgPool,
    id: &str,
    started_at: DateTime<Utc>,
    deadline_at: DateTime<Utc>,
    outcome: &str,
) {
    let completed_at = (outcome != "running").then_some(started_at + Duration::seconds(1));
    sqlx::query(
        "insert into model_requests (
           id, client_api_key_ref, config_revision, protocol, operation, endpoint,
           client_transport, requested_model_id, outcome,
           started_at, deadline_at, completed_at,
           routing_scope, routing_group_refs, routing_group_names_snapshot
         ) values (
           $1, 'key-recovery', 1, 'openai', 'responses', '/v1/responses',
           'http_sse', 'coding', $2, $3, $4, $5,
           'all', '{}'::text[], '[]'::jsonb
         )",
    )
    .bind(id)
    .bind(outcome)
    .bind(started_at)
    .bind(deadline_at)
    .bind(completed_at)
    .execute(pool)
    .await
    .expect("seed model request recovery fact");
}

#[tokio::test]
async fn recovery_aggregates_frozen_customer_refs_after_key_reassignment_and_deletion() {
    let Some(database) = TestDatabase::create("customer_admission_recovery").await else {
        return;
    };
    let now = Utc::now();
    sqlx::raw_sql(
        "insert into customers (id, name) values ('cust_original', 'Original'), ('cust_new', 'New');
         insert into client_api_keys (id, name, key, customer_id, created_at, updated_at)
         values ('key-recovery', 'first', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'cust_original', now(), now());"
    ).execute(&database.pool).await.expect("seed customer and key");
    let error = sqlx::query("delete from customers where id = 'cust_original'")
        .execute(&database.pool)
        .await
        .expect_err("cannot delete a customer with keys");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23001")
    );
    seed_request(
        &database.pool,
        "req_first_customer",
        now,
        now + Duration::seconds(60),
        "running",
    )
    .await;
    seed_request(
        &database.pool,
        "req_second_customer",
        now,
        now + Duration::seconds(60),
        "running",
    )
    .await;
    sqlx::raw_sql(
        "update model_requests set customer_ref = 'cust_original';
         update model_requests set client_api_key_ref = 'key-other' where id = 'req_second_customer';
         update client_api_keys set customer_id = 'cust_new' where id = 'key-recovery';
         delete from customers where id = 'cust_original';
         delete from client_api_keys where id = 'key-recovery';"
    ).execute(&database.pool).await.expect("change live ownership and remove original records");
    let recoveries = PgClientAdmissionRecoveryRepository::new(database.pool.clone())
        .load_client_admission_recovery(now - Duration::seconds(60))
        .await
        .expect("load frozen recovery");
    assert_eq!(recoveries.len(), 4);
    let original =
        AdmissionScopeId::Customer(CustomerId::new("cust_original").expect("customer ID"));
    let customer = recoveries
        .iter()
        .find(|recovery| recovery.scope_id == original)
        .expect("original customer recovery");
    assert_eq!(customer.recent_requests.len(), 2);
    assert_eq!(customer.running_requests.len(), 2);
    assert!(!recoveries.iter().any(|recovery| recovery.scope_id
        == AdmissionScopeId::Customer(CustomerId::new("cust_new").expect("customer ID"))));
    database.close().await;
}

#[tokio::test]
async fn recovery_uses_access_group_at_admission_after_reassignment_and_deletion() {
    use gateway_core::policy::AccessGroupId;
    let Some(database) = TestDatabase::create("access_recovery").await else {
        return;
    };
    let now = Utc::now();
    sqlx::raw_sql(
        "insert into access_groups (id, name) values ('access_old', 'Old'), ('access_new', 'New');
         insert into client_api_keys (id, name, key, access_group_id, created_at, updated_at)
         values ('key-recovery', 'Key', 'sk_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ', 'access_old', now(), now());"
    ).execute(&database.pool).await.expect("seed group and key");
    seed_request(
        &database.pool,
        "req_group_first",
        now,
        now + Duration::seconds(60),
        "running",
    )
    .await;
    seed_request(
        &database.pool,
        "req_group_second",
        now,
        now + Duration::seconds(60),
        "succeeded",
    )
    .await;
    sqlx::raw_sql(
        "update model_requests set access_group_ref = 'access_old';
         update model_requests set client_api_key_ref = 'key-other' where id = 'req_group_second';
         update client_api_keys set access_group_id = 'access_new' where id = 'key-recovery';
         delete from access_groups where id = 'access_old';
         delete from client_api_keys where id = 'key-recovery';",
    )
    .execute(&database.pool)
    .await
    .expect("reassign and remove live records");
    let recoveries = PgClientAdmissionRecoveryRepository::new(database.pool.clone())
        .load_client_admission_recovery(now - Duration::seconds(60))
        .await
        .expect("recovery");
    assert_eq!(recoveries.len(), 4);
    let original = AdmissionScopeId::AccessGroup(AccessGroupId::new("access_old").expect("group"));
    let group = recoveries
        .iter()
        .find(|recovery| recovery.scope_id == original)
        .expect("original group");
    assert_eq!(group.recent_requests.len(), 2);
    assert_eq!(group.running_requests.len(), 1);
    assert!(!recoveries.iter().any(|recovery| recovery.scope_id
        == AdmissionScopeId::AccessGroup(AccessGroupId::new("access_new").expect("group"))));
    database.close().await;
}
