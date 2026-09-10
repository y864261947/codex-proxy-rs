use gateway_admin::model::{MutationActor, MutationContext};
use gateway_core::policy::RateLimits;
use gateway_store::postgres::{
    PgControlPlaneRepository, PgRuntimeSettingsRepository, PgRuntimeSnapshotRepository,
    RuntimeSettingsRepository, RuntimeSettingsUpdate, RuntimeSnapshotRepository,
};

use super::TestDatabase;

#[tokio::test]
async fn global_settings_are_atomic_audited_and_preserved_by_legacy_settings_updates() {
    let Some(db) = TestDatabase::create("global_settings").await else {
        return;
    };
    let repository = PgControlPlaneRepository::new(db.pool.clone());
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "req_global_settings".to_owned(),
    };
    let original = repository
        .load_global_admission()
        .await
        .expect("load default");
    assert_eq!(original.limits, RateLimits::unlimited());
    let limits = RateLimits {
        max_concurrency: 12,
        requests_per_minute: 500,
    };
    let saved = repository
        .replace_global_admission(limits, &context)
        .await
        .expect("save limits");
    assert_eq!(
        saved.config_revision.get(),
        original.config_revision.get() + 1
    );
    assert_eq!(
        repository
            .load_global_admission()
            .await
            .expect("load saved"),
        saved
    );
    let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .expect("snapshot");
    assert_eq!(snapshot.settings.global_limits, limits);
    let audit: Vec<Vec<String>> = sqlx::query_scalar(
        "select changed_fields from admin_audit_events order by config_revision",
    )
    .fetch_all(&db.pool)
    .await
    .expect("audit");
    assert_eq!(
        audit,
        [vec![
            "global_max_concurrency".to_owned(),
            "global_requests_per_minute".to_owned()
        ]]
    );
    let invalid_context = MutationContext {
        actor: MutationActor::AdminSession {
            admin_user_id: "missing_admin".to_owned(),
        },
        request_id: "req_missing_actor".to_owned(),
    };
    assert!(
        repository
            .replace_global_admission(RateLimits::unlimited(), &invalid_context)
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .load_global_admission()
            .await
            .expect("audit failure rolled back"),
        saved
    );
    assert!(
        repository
            .replace_global_admission(
                RateLimits {
                    max_concurrency: u64::MAX,
                    requests_per_minute: 0
                },
                &context
            )
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .load_global_admission()
            .await
            .expect("invalid limits did not mutate"),
        saved
    );

    let settings = PgRuntimeSettingsRepository::new(db.pool.clone());
    let old = settings
        .load_runtime_settings()
        .await
        .expect("legacy settings");
    settings
        .update_runtime_settings(RuntimeSettingsUpdate {
            admin_api_key: old.admin_api_key,
            refresh_margin_seconds: old.refresh_margin_seconds,
            refresh_concurrency: old.refresh_concurrency,
            max_concurrent_per_account: old.max_concurrent_per_account,
            request_interval_ms: old.request_interval_ms,
            rotation_strategy: old.rotation_strategy,
            model_mappings: old.model_mappings,
            min_codex_desktop_version: old.min_codex_desktop_version,
            min_codex_cli_version: old.min_codex_cli_version,
            usage_retention_days: old.usage_retention_days,
            ops_event_retention_days: old.ops_event_retention_days,
            audit_retention_days: old.audit_retention_days,
        })
        .await
        .expect("old client saves settings");
    assert_eq!(
        repository
            .load_global_admission()
            .await
            .expect("limits preserved")
            .limits,
        limits
    );
    db.close().await;
}
