use gateway_admin::{
    model::{
        MutationActor, MutationContext, PageSize,
        channels::{ChannelChange, ChannelFields, ChannelListQuery},
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
