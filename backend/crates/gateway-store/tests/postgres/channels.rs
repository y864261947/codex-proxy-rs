use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        channels::{
            ChannelChange, ChannelDiscoveryComparisonQuery, ChannelDiscoveryQuery, ChannelFields,
            ChannelListQuery, ChannelModelPreview,
        },
    },
    ports::store::{AdminStoreErrorKind, ChannelStore},
};
use gateway_core::{
    channel::{ChannelRevision, ChannelStorePort, ProviderChannelConfig},
    identity::{ChannelId, ProviderKind, QuotaScopeId},
    policy::RateLimits,
    provider_ports::ProviderStoreErrorKind,
    routing::source::SourcePreference,
};
use gateway_store::postgres::PgChannelRepository;
use serde_json::{Map, Value};

use super::TestDatabase;

fn discovery(generation: u64, connection_revision: u64) -> ChannelModelPreview {
    ChannelModelPreview {
        id: id(),
        revision: revision(connection_revision),
        generation,
        fetched_at: chrono::DateTime::from_timestamp(1_789_099_200, 0).expect("time"),
        added: vec!["new-model".to_owned()],
        missing: vec!["missing-model".to_owned()],
        unchanged: vec!["existing".to_owned()],
    }
}

#[tokio::test]
async fn model_discovery_is_durable_version_fenced_and_ordered_without_config_mutations() {
    let Some(db) = TestDatabase::create("channel_discovery").await else {
        return;
    };
    let repo = PgChannelRepository::new(db.pool.clone());
    let mut channel_fields = fields();
    channel_fields.quota_scope_id = None;
    let created = repo
        .change_channel(
            ChannelChange::Create {
                id: id(),
                provider: provider(),
                fields: channel_fields.clone(),
                config: config("private-token"),
            },
            &context(),
        )
        .await
        .expect("create");
    assert!(
        repo.load_model_discovery(&id())
            .await
            .expect("empty history")
            .is_none()
    );
    let first = repo
        .reserve_model_discovery(&id(), revision(1))
        .await
        .expect("first query");
    let second = repo
        .reserve_model_discovery(&id(), revision(1))
        .await
        .expect("second query");
    assert!(second > first);
    let newest = discovery(second, 1);
    repo.save_model_discovery(&newest)
        .await
        .expect("newer finished first");
    assert_eq!(
        repo.save_model_discovery(&discovery(first, 1))
            .await
            .expect_err("late result rejected")
            .kind(),
        AdminStoreErrorKind::StaleRevision
    );
    let reopened = PgChannelRepository::new(db.pool.clone());
    assert_eq!(
        reopened
            .load_model_discovery(&id())
            .await
            .expect("persisted"),
        Some(newest.clone())
    );
    assert_eq!(
        repo.list_channels(query())
            .await
            .expect("config unchanged")
            .config_revision,
        created
    );
    let audit_count: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .expect("audit count");
    assert_eq!(audit_count, 1);
    let third = repo
        .reserve_model_discovery(&id(), revision(1))
        .await
        .expect("third query");
    sqlx::query("alter table channel_model_discoveries add constraint reject_discovery_write check (generation < 0) not valid")
        .execute(&db.pool).await.expect("simulate failed write");
    assert!(
        repo.save_model_discovery(&discovery(third, 1))
            .await
            .is_err()
    );
    assert_eq!(
        repo.load_model_discovery(&id())
            .await
            .expect("failure retained success"),
        Some(newest.clone())
    );
    sqlx::query("alter table channel_model_discoveries drop constraint reject_discovery_write")
        .execute(&db.pool)
        .await
        .expect("recover");
    repo.change_channel(
        ChannelChange::Update {
            id: id(),
            expected_revision: revision(1),
            fields: channel_fields,
            replacement_config: Some(config("rotated-token")),
        },
        &context(),
    )
    .await
    .expect("rotate");
    assert_eq!(
        repo.save_model_discovery(&discovery(third, 1))
            .await
            .expect_err("rotation during query")
            .kind(),
        AdminStoreErrorKind::StaleRevision
    );
    assert_eq!(
        repo.reserve_model_discovery(&id(), revision(1))
            .await
            .expect_err("stale start")
            .kind(),
        AdminStoreErrorKind::StaleRevision
    );
    assert_eq!(
        repo.load_model_discovery(&id())
            .await
            .expect("old version still readable"),
        Some(newest)
    );
    let next = repo
        .reserve_model_discovery(&id(), revision(2))
        .await
        .expect("new version query");
    let mut empty = discovery(next, 2);
    empty.added.clear();
    empty.unchanged.clear();
    repo.save_model_discovery(&empty)
        .await
        .expect("empty upstream success persisted");
    assert_eq!(
        repo.load_model_discovery(&id())
            .await
            .expect("empty snapshot distinct from no history"),
        Some(empty)
    );
    repo.change_channel(
        ChannelChange::Delete {
            id: id(),
            expected_revision: revision(2),
        },
        &context(),
    )
    .await
    .expect("delete channel");
    let count: i64 = sqlx::query_scalar("select count(*) from channel_model_discoveries")
        .fetch_one(&db.pool)
        .await
        .expect("cascaded");
    assert_eq!(count, 0);
    assert_eq!(
        repo.load_model_discovery(&id())
            .await
            .expect_err("deleted channel")
            .kind(),
        AdminStoreErrorKind::NotFound
    );
    db.close().await;
}

#[tokio::test]
async fn concurrent_discovery_saves_keep_the_highest_query_generation() {
    let Some(db) = TestDatabase::create("discovery_race").await else {
        return;
    };
    let repo = PgChannelRepository::new(db.pool.clone());
    let mut channel_fields = fields();
    channel_fields.quota_scope_id = None;
    repo.change_channel(
        ChannelChange::Create {
            id: id(),
            provider: provider(),
            fields: channel_fields,
            config: config("fixture-only"),
        },
        &context(),
    )
    .await
    .expect("create");
    let first = discovery(
        repo.reserve_model_discovery(&id(), revision(1))
            .await
            .expect("first"),
        1,
    );
    let second = discovery(
        repo.reserve_model_discovery(&id(), revision(1))
            .await
            .expect("second"),
        1,
    );
    let (older, newer) = tokio::join!(
        repo.save_model_discovery(&first),
        repo.save_model_discovery(&second)
    );
    newer.expect("newer always accepted");
    if let Err(error) = older {
        assert_eq!(error.kind(), AdminStoreErrorKind::StaleRevision);
    }
    assert_eq!(
        repo.load_model_discovery(&id()).await.expect("latest"),
        Some(second)
    );
    db.close().await;
}

#[tokio::test]
async fn discovery_history_migration_preserves_latest_and_pages_by_channel_and_generation() {
    let Some(db) = TestDatabase::create("discovery_history").await else {
        return;
    };
    let repo = PgChannelRepository::new(db.pool.clone());
    let mut channel_fields = fields();
    channel_fields.quota_scope_id = None;
    repo.change_channel(
        ChannelChange::Create {
            id: id(),
            provider: provider(),
            fields: channel_fields.clone(),
            config: config("fixture-only"),
        },
        &context(),
    )
    .await
    .expect("create");
    let mut history_query = ChannelDiscoveryQuery {
        id: id(),
        before_generation: None,
        page_size: PageSize::new(1).expect("size"),
    };
    assert!(
        repo.list_model_discoveries(history_query.clone())
            .await
            .expect("no history")
            .items
            .is_empty()
    );
    sqlx::raw_sql("alter table channel_model_discoveries drop constraint channel_model_discoveries_pkey, add primary key (channel_id)")
        .execute(&db.pool).await.expect("restore prior schema in isolated fixture");
    let first = discovery(
        repo.reserve_model_discovery(&id(), revision(1))
            .await
            .expect("first generation"),
        1,
    );
    repo.save_model_discovery(&first)
        .await
        .expect("single legacy record");
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0012_channel_model_discovery_history.sql"
    ))
    .execute(&db.pool)
    .await
    .expect("upgrade preserves record");
    assert_eq!(
        repo.load_model_discovery(&id()).await.expect("preserved"),
        Some(first.clone())
    );
    let second = discovery(
        repo.reserve_model_discovery(&id(), revision(1))
            .await
            .expect("second generation"),
        1,
    );
    repo.save_model_discovery(&second)
        .await
        .expect("append same version");
    let first_page = repo
        .list_model_discoveries(history_query.clone())
        .await
        .expect("first page");
    assert_eq!(first_page.items, vec![second.clone()]);
    assert_eq!(first_page.next_before_generation, Some(second.generation));
    repo.change_channel(
        ChannelChange::Update {
            id: id(),
            expected_revision: revision(1),
            fields: channel_fields.clone(),
            replacement_config: Some(config("rotated-fixture")),
        },
        &context(),
    )
    .await
    .expect("new revision");
    let third = discovery(
        repo.reserve_model_discovery(&id(), revision(2))
            .await
            .expect("third generation"),
        2,
    );
    repo.save_model_discovery(&third)
        .await
        .expect("insert between page reads");
    history_query.before_generation = first_page.next_before_generation;
    let older = repo
        .list_model_discoveries(history_query.clone())
        .await
        .expect("older page");
    assert_eq!(older.items, vec![first.clone()]);
    assert_eq!(older.next_before_generation, None);
    history_query.before_generation = Some(first.generation);
    assert!(
        repo.list_model_discoveries(history_query.clone())
            .await
            .expect("end of history")
            .items
            .is_empty()
    );
    assert_eq!(
        repo.load_model_discovery(&id()).await.expect("latest"),
        Some(third.clone())
    );

    let other = ChannelId::new("chan_other").expect("other");
    channel_fields.name = "Other channel".to_owned();
    repo.change_channel(
        ChannelChange::Create {
            id: other.clone(),
            provider: provider(),
            fields: channel_fields,
            config: config("other-fixture"),
        },
        &context(),
    )
    .await
    .expect("other channel");
    let mut other_record = discovery(
        repo.reserve_model_discovery(&other, revision(1))
            .await
            .expect("other generation"),
        1,
    );
    other_record.id = other.clone();
    repo.save_model_discovery(&other_record)
        .await
        .expect("other history");
    history_query.before_generation = None;
    history_query.page_size = PageSize::new(50).expect("size");
    let all = repo
        .list_model_discoveries(history_query.clone())
        .await
        .expect("only this channel");
    assert_eq!(all.items, vec![third, second, first]);
    assert_eq!(all.next_before_generation, None);
    let comparison_query = ChannelDiscoveryComparisonQuery {
        id: id(),
        base_generation: all.items[2].generation,
        target_generation: all.items[0].generation,
    };
    let pair = repo
        .load_discovery_pair(comparison_query.clone())
        .await
        .expect("load exact pair")
        .expect("pair");
    assert_eq!(pair.base, all.items[2]);
    assert_eq!(pair.target, all.items[0]);
    assert_ne!(pair.base.revision, pair.target.revision);
    let mut cross_channel = comparison_query.clone();
    cross_channel.target_generation = other_record.generation;
    assert!(
        repo.load_discovery_pair(cross_channel)
            .await
            .expect("cannot read other channel record")
            .is_none()
    );
    let rejected = discovery(
        repo.reserve_model_discovery(&id(), revision(2))
            .await
            .expect("next generation"),
        2,
    );
    sqlx::query("alter table channel_model_discoveries add constraint reject_history_write check (generation < 0) not valid")
        .execute(&db.pool).await.expect("simulate write failure");
    assert!(repo.save_model_discovery(&rejected).await.is_err());
    assert_eq!(
        repo.list_model_discoveries(history_query.clone())
            .await
            .expect("all history retained")
            .items,
        all.items
    );
    sqlx::query("alter table channel_model_discoveries drop constraint reject_history_write")
        .execute(&db.pool)
        .await
        .expect("recover writes");
    repo.change_channel(
        ChannelChange::Delete {
            id: id(),
            expected_revision: revision(2),
        },
        &context(),
    )
    .await
    .expect("delete channel");
    assert_eq!(
        repo.list_model_discoveries(history_query.clone())
            .await
            .expect_err("deleted channel")
            .kind(),
        AdminStoreErrorKind::NotFound
    );
    history_query.id = other;
    assert!(
        repo.load_discovery_pair(comparison_query)
            .await
            .expect("deleted comparison")
            .is_none()
    );
    assert_eq!(
        repo.list_model_discoveries(history_query)
            .await
            .expect("other retained")
            .items,
        vec![other_record]
    );
    let count: i64 = sqlx::query_scalar("select count(*) from channel_model_discoveries")
        .fetch_one(&db.pool)
        .await
        .expect("cascade all versions");
    assert_eq!(count, 1);
    db.close().await;
}

fn id() -> ChannelId {
    ChannelId::new("chan_test").expect("channel")
}
fn provider() -> ProviderKind {
    ProviderKind::new("openai_api").expect("provider")
}
fn revision(value: u64) -> ChannelRevision {
    ChannelRevision::new(value).expect("revision")
}
fn config(secret: &str) -> ProviderChannelConfig {
    ProviderChannelConfig::new(Map::from_iter([(
        "token".to_owned(),
        Value::String(secret.to_owned()),
    )]))
    .expect("Provider-prepared config")
}
fn fields() -> ChannelFields {
    ChannelFields {
        name: "100% Channel".to_owned(),
        note: Some("admin note".to_owned()),
        enabled: true,
        preference: SourcePreference::new(2, 5).expect("preference"),
        limits: RateLimits {
            max_concurrency: 4,
            requests_per_minute: 60,
        },
        quota_scope_id: Some(QuotaScopeId::new("quota_shared_project").expect("quota")),
    }
}
fn query() -> ChannelListQuery {
    ChannelListQuery {
        page: 1,
        page_size: PageSize::new(20).expect("page"),
        search: None,
        provider: None,
    }
}
fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: "req_channel_admin".to_owned(),
    }
}

#[tokio::test]
async fn channel_rotation_is_revision_fenced_and_public_reads_and_audits_do_not_expose_config() {
    let Some(db) = TestDatabase::create("channel_rotation").await else {
        return;
    };
    sqlx::query("insert into upstream_quota_scopes (id, name) values ('quota_shared_project', 'Shared project')").execute(&db.pool).await.expect("shared quota fixture");
    let repo = PgChannelRepository::new(db.pool.clone());
    let created = repo
        .change_channel(
            ChannelChange::Create {
                id: id(),
                provider: provider(),
                fields: fields(),
                config: config("first-sensitive-token"),
            },
            &context(),
        )
        .await
        .expect("create");
    let page = repo.list_channels(query()).await.expect("list");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].fields, fields());
    assert_eq!(page.items[0].connection_revision, revision(1));
    assert_eq!(page.config_revision, created);
    assert!(!format!("{page:?}").contains("sensitive-token"));
    let mut search = query();
    search.search = Some("%".to_owned());
    assert_eq!(
        repo.list_channels(search)
            .await
            .expect("literal search")
            .total,
        1
    );
    let mut filtered = query();
    filtered.provider = Some(ProviderKind::new("different_api").expect("provider"));
    assert_eq!(
        repo.list_channels(filtered)
            .await
            .expect("provider filter")
            .total,
        0
    );
    assert_eq!(
        repo.list_enabled_channels(&provider())
            .await
            .expect("Provider catalog inputs")
            .len(),
        1
    );
    let frozen = repo
        .load_channel(&id(), revision(1))
        .await
        .expect("load")
        .expect("channel");
    assert_eq!(
        frozen.config.expose_to_provider()["token"],
        "first-sensitive-token"
    );
    let rotated = repo
        .change_channel(
            ChannelChange::Update {
                id: id(),
                expected_revision: revision(1),
                fields: fields(),
                replacement_config: Some(config("second-sensitive-token")),
            },
            &context(),
        )
        .await
        .expect("rotate");
    assert_eq!(rotated.get(), created.get() + 1);
    assert_eq!(
        repo.load_channel(&id(), revision(1))
            .await
            .expect_err("old candidate cannot use new credentials")
            .kind(),
        ProviderStoreErrorKind::Conflict
    );
    assert_eq!(
        frozen.config.expose_to_provider()["token"],
        "first-sensitive-token"
    );
    assert_eq!(
        repo.load_channel(&id(), revision(2))
            .await
            .expect("load current")
            .expect("channel")
            .config
            .expose_to_provider()["token"],
        "second-sensitive-token"
    );
    let stale = repo
        .change_channel(
            ChannelChange::Update {
                id: id(),
                expected_revision: revision(1),
                fields: fields(),
                replacement_config: Some(config("lost-update-token")),
            },
            &context(),
        )
        .await
        .expect_err("stale update");
    assert_eq!(stale.kind(), AdminStoreErrorKind::StaleRevision);
    assert_eq!(
        repo.list_channels(query())
            .await
            .expect("unchanged")
            .config_revision,
        rotated
    );
    let mut disabled = fields();
    disabled.enabled = false;
    disabled.name = "Renamed channel".to_owned();
    repo.change_channel(
        ChannelChange::Update {
            id: id(),
            expected_revision: revision(2),
            fields: disabled,
            replacement_config: None,
        },
        &context(),
    )
    .await
    .expect("disable and rename without replacing token");
    assert!(
        repo.load_channel(&id(), revision(3))
            .await
            .expect("disabled")
            .is_none()
    );
    let editable = repo
        .load_channel_for_edit(&id())
        .await
        .expect("disabled channel can be edited")
        .expect("editable");
    assert_eq!(editable.revision, revision(3));
    assert_eq!(
        editable.config.expose_to_provider()["token"],
        "second-sensitive-token"
    );
    assert!(
        repo.list_enabled_channels(&provider())
            .await
            .expect("disabled catalog")
            .is_empty()
    );
    let retained: Value =
        sqlx::query_scalar("select provider_config_json from upstream_channels where id=$1")
            .bind(id().as_str())
            .fetch_one(&db.pool)
            .await
            .expect("stored config");
    assert_eq!(retained["token"], "second-sensitive-token");
    let audit: String =
        sqlx::query_scalar("select jsonb_agg(to_jsonb(a))::text from admin_audit_events a")
            .fetch_one(&db.pool)
            .await
            .expect("audit");
    assert!(!audit.contains("sensitive-token"));
    assert!(!audit.contains("lost-update-token"));
    assert!(audit.contains("connection"));
    assert_eq!(
        repo.change_channel(
            ChannelChange::Delete {
                id: id(),
                expected_revision: revision(2)
            },
            &context()
        )
        .await
        .expect_err("stale delete")
        .kind(),
        AdminStoreErrorKind::StaleRevision
    );
    repo.change_channel(
        ChannelChange::Delete {
            id: id(),
            expected_revision: revision(3),
        },
        &context(),
    )
    .await
    .expect("delete");
    assert!(
        repo.load_channel(&id(), revision(3))
            .await
            .expect("deleted")
            .is_none()
    );
    assert_eq!(repo.list_channels(query()).await.expect("empty").total, 0);
    assert_eq!(
        repo.change_channel(
            ChannelChange::Delete {
                id: id(),
                expected_revision: revision(3)
            },
            &context()
        )
        .await
        .expect_err("missing")
        .kind(),
        AdminStoreErrorKind::NotFound
    );
    db.close().await;
}

#[tokio::test]
async fn channel_conflicts_and_audit_failure_roll_back_config_and_revision() {
    let Some(db) = TestDatabase::create("channel_rollback").await else {
        return;
    };
    sqlx::query("insert into upstream_quota_scopes (id, name) values ('quota_shared_project', 'Shared project')").execute(&db.pool).await.expect("shared quota fixture");
    let repo = PgChannelRepository::new(db.pool.clone());
    let initial = repo
        .change_channel(
            ChannelChange::Create {
                id: id(),
                provider: provider(),
                fields: fields(),
                config: config("original-token"),
            },
            &context(),
        )
        .await
        .expect("create");
    let duplicate = repo
        .change_channel(
            ChannelChange::Create {
                id: ChannelId::new("chan_duplicate").expect("channel"),
                provider: provider(),
                fields: fields(),
                config: config("duplicate-token"),
            },
            &context(),
        )
        .await
        .expect_err("unique name");
    assert_eq!(duplicate.kind(), AdminStoreErrorKind::Conflict);
    assert_eq!(
        repo.list_channels(query())
            .await
            .expect("unchanged")
            .config_revision,
        initial
    );
    sqlx::query("alter table admin_audit_events add constraint channel_audit_test_reject check (entity_kind <> 'channel') not valid").execute(&db.pool).await.expect("force audit failure");
    assert!(
        repo.change_channel(
            ChannelChange::Update {
                id: id(),
                expected_revision: revision(1),
                fields: fields(),
                replacement_config: Some(config("uncommitted-token"))
            },
            &context()
        )
        .await
        .is_err()
    );
    let page = repo.list_channels(query()).await.expect("rolled back");
    assert_eq!(page.config_revision, initial);
    assert_eq!(page.items[0].connection_revision, revision(1));
    assert_eq!(
        repo.load_channel(&id(), revision(1))
            .await
            .expect("load original")
            .expect("channel")
            .config
            .expose_to_provider()["token"],
        "original-token"
    );
    let audit_count: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .expect("audit count");
    assert_eq!(audit_count, 1);
    db.close().await;
}
