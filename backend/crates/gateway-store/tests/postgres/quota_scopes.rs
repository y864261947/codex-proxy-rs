use super::TestDatabase;
use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        quota_scopes::{QuotaScopeChange, QuotaScopeFields, QuotaScopeListQuery},
    },
    ports::store::{AdminStoreErrorKind, QuotaScopeStore},
};
use gateway_core::{identity::QuotaScopeId, policy::RateLimits};
use gateway_store::postgres::{
    PgQuotaScopeRepository, PgRuntimeSnapshotRepository, RuntimeSnapshotRepository,
};

fn id(value: &str) -> QuotaScopeId {
    QuotaScopeId::new(value).expect("quota ID")
}
fn fields(name: &str) -> QuotaScopeFields {
    QuotaScopeFields {
        name: name.to_owned(),
        note: Some("private operator note".to_owned()),
        enabled: true,
        limits: RateLimits {
            max_concurrency: 3,
            requests_per_minute: 90,
        },
    }
}
fn query() -> QuotaScopeListQuery {
    QuotaScopeListQuery {
        page: 1,
        page_size: PageSize::new(20).expect("page size"),
        search: None,
    }
}
fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: "req_quota_admin".to_owned(),
    }
}

#[tokio::test]
async fn quota_management_counts_both_source_types_and_commits_snapshot_with_safe_audit() {
    let Some(db) = TestDatabase::create("quota_admin").await else {
        return;
    };
    let store = PgQuotaScopeRepository::new(db.pool.clone());
    let created = store
        .change_quota_scope(
            QuotaScopeChange::Create {
                id: id("quota_shared"),
                fields: fields("Shared 100%"),
            },
            &context(),
        )
        .await
        .expect("create");
    sqlx::query("insert into upstream_channels (id,provider_kind,name,quota_scope_id,provider_config_json) values ('chan_shared','openai_api','A','quota_shared','{\"token\":\"test\"}'::jsonb)").execute(&db.pool).await.expect("channel");
    sqlx::query("insert into account_groups (id,name,quota_scope_id,created_at,updated_at,color) values ('grp_00000000000000000000000000000071','Pool','quota_shared',now(),now(),'#3B82F6FF')").execute(&db.pool).await.expect("pool");
    let page = store.list_quota_scopes(query()).await.expect("page");
    assert_eq!(page.config_revision, created);
    assert_eq!(page.items[0].source_count, 2);
    let mut search = query();
    search.search = Some("100%".to_owned());
    assert_eq!(
        store
            .list_quota_scopes(search)
            .await
            .expect("literal search")
            .total,
        1
    );
    let original = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("original snapshot");
    let mut changed = fields("Renamed");
    changed.enabled = false;
    changed.limits.max_concurrency = 1;
    let updated = store
        .change_quota_scope(
            QuotaScopeChange::Update {
                id: id("quota_shared"),
                fields: changed,
            },
            &context(),
        )
        .await
        .expect("update");
    assert!(updated.get() > created.get());
    let current = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("new snapshot");
    assert!(!current.quotas[0].enabled());
    assert_eq!(current.quotas[0].limits().max_concurrency, 1);
    assert!(original.quotas[0].enabled());
    let audits: Vec<String> = sqlx::query_scalar(
        "select row_to_json(a)::text from admin_audit_events a order by config_revision",
    )
    .fetch_all(&db.pool)
    .await
    .expect("audits");
    assert_eq!(audits.len(), 2);
    assert!(
        audits
            .iter()
            .all(|audit| audit.contains("quota_scope") && !audit.contains("private operator note"))
    );
    db.close().await;
}

#[tokio::test]
async fn quota_conflicts_and_audit_failure_rollback_config_revision_and_fields() {
    let Some(db) = TestDatabase::create("quota_admin_rollback").await else {
        return;
    };
    let store = PgQuotaScopeRepository::new(db.pool.clone());
    store
        .change_quota_scope(
            QuotaScopeChange::Create {
                id: id("quota_shared"),
                fields: fields("Shared"),
            },
            &context(),
        )
        .await
        .expect("create");
    sqlx::query("insert into account_groups (id,name,quota_scope_id,created_at,updated_at,color) values ('grp_00000000000000000000000000000072','Pool','quota_shared',now(),now(),'#3B82F6FF')").execute(&db.pool).await.expect("pool");
    let before = store.list_quota_scopes(query()).await.expect("before");
    for command in [
        QuotaScopeChange::Create {
            id: id("quota_duplicate"),
            fields: fields("Shared"),
        },
        QuotaScopeChange::Delete {
            id: id("quota_shared"),
        },
    ] {
        assert_eq!(
            store
                .change_quota_scope(command, &context())
                .await
                .expect_err("conflict")
                .kind(),
            AdminStoreErrorKind::Conflict
        );
        assert_eq!(
            store.list_quota_scopes(query()).await.expect("unchanged"),
            before
        );
    }
    assert_eq!(
        store
            .change_quota_scope(
                QuotaScopeChange::Update {
                    id: id("quota_missing"),
                    fields: fields("Missing")
                },
                &context()
            )
            .await
            .expect_err("missing")
            .kind(),
        AdminStoreErrorKind::NotFound
    );
    sqlx::query("alter table admin_audit_events add constraint reject_quota_audit check (entity_kind <> 'quota_scope') not valid").execute(&db.pool).await.expect("reject future audit");
    assert!(
        store
            .change_quota_scope(
                QuotaScopeChange::Update {
                    id: id("quota_shared"),
                    fields: fields("Must rollback")
                },
                &context()
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .list_quota_scopes(query())
            .await
            .expect("audit rollback"),
        before
    );
    let audits: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .expect("audit count");
    assert_eq!(audits, 1);
    sqlx::query("alter table admin_audit_events drop constraint reject_quota_audit")
        .execute(&db.pool)
        .await
        .expect("restore audit");
    sqlx::query("update account_groups set quota_scope_id=null")
        .execute(&db.pool)
        .await
        .expect("unlink pool");
    store
        .change_quota_scope(
            QuotaScopeChange::Delete {
                id: id("quota_shared"),
            },
            &context(),
        )
        .await
        .expect("delete unreferenced quota");
    assert_eq!(
        store.list_quota_scopes(query()).await.expect("empty").total,
        0
    );
    db.close().await;
}
