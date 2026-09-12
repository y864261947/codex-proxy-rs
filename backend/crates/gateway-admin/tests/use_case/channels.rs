use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_admin::{
    model::{
        AdminErrorKind, MutationActor, MutationContext, Revision,
        channels::{
            ChannelChange, ChannelDiscoveryComparisonQuery, ChannelDiscoveryPage,
            ChannelDiscoveryPair, ChannelDiscoveryQuery, ChannelFields, ChannelListQuery,
            ChannelModelPreview, ChannelPage, NewChannel, UpdateChannel,
        },
        provider_credentials::ProviderDocument,
    },
    ports::{
        channels::ChannelProviderAdmin,
        provider::{ProviderAdminError, ProviderAdminErrorKind},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ChannelStore},
    },
};
use gateway_core::{
    account::OpaqueProviderData,
    channel::{ChannelRevision, ProviderChannelConfig, StoredChannel},
    identity::{ChannelId, ProviderKind},
    policy::RateLimits,
    routing::{ConfigRevision, source::SourcePreference},
    runtime::SnapshotControl,
};
use serde_json::{Value, json};

use super::{AdminHarness, UnavailableStore, unavailable};

#[async_trait]
impl ChannelStore for UnavailableStore {
    async fn load_discovery_pair(
        &self,
        _: ChannelDiscoveryComparisonQuery,
    ) -> AdminStoreResult<Option<ChannelDiscoveryPair>> {
        Err(unavailable("channel"))
    }
    async fn list_model_discoveries(
        &self,
        _: ChannelDiscoveryQuery,
    ) -> AdminStoreResult<ChannelDiscoveryPage> {
        Err(unavailable("channel"))
    }
    async fn reserve_model_discovery(
        &self,
        _: &ChannelId,
        _: ChannelRevision,
    ) -> AdminStoreResult<u64> {
        Err(unavailable("channel"))
    }
    async fn save_model_discovery(&self, _: &ChannelModelPreview) -> AdminStoreResult<()> {
        Err(unavailable("channel"))
    }
    async fn load_model_discovery(
        &self,
        _: &ChannelId,
    ) -> AdminStoreResult<Option<ChannelModelPreview>> {
        Err(unavailable("channel"))
    }
    async fn list_channels(&self, _: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        Err(unavailable("channel"))
    }
    async fn load_channel_for_edit(
        &self,
        _: &ChannelId,
    ) -> AdminStoreResult<Option<StoredChannel>> {
        Err(unavailable("channel"))
    }
    async fn change_channel(
        &self,
        _: ChannelChange,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        Err(unavailable("channel"))
    }
}

#[derive(Default)]
struct MemoryChannels {
    state: Mutex<ChannelState>,
}
#[derive(Default)]
struct ChannelState {
    stored: Option<StoredChannel>,
    commits: u64,
    reject: bool,
    generation: u64,
    discoveries: Vec<ChannelModelPreview>,
}

#[async_trait]
impl ChannelStore for MemoryChannels {
    async fn load_discovery_pair(
        &self,
        query: ChannelDiscoveryComparisonQuery,
    ) -> AdminStoreResult<Option<ChannelDiscoveryPair>> {
        let state = self.state.lock().expect("state");
        if state.reject {
            return Err(unavailable("channel"));
        }
        let find = |generation| {
            state
                .discoveries
                .iter()
                .find(|item| item.id == query.id && item.generation == generation)
                .cloned()
        };
        Ok(find(query.base_generation)
            .zip(find(query.target_generation))
            .map(|(base, target)| ChannelDiscoveryPair { base, target }))
    }
    async fn list_model_discoveries(
        &self,
        query: ChannelDiscoveryQuery,
    ) -> AdminStoreResult<ChannelDiscoveryPage> {
        let state = self.state.lock().expect("state");
        if state.reject {
            return Err(unavailable("channel"));
        }
        if !state
            .stored
            .as_ref()
            .is_some_and(|stored| stored.id == query.id)
        {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "channel",
                "missing",
            ));
        }
        let mut items: Vec<_> = state
            .discoveries
            .iter()
            .rev()
            .filter(|item| {
                query
                    .before_generation
                    .is_none_or(|before| item.generation < before)
            })
            .take(usize::from(query.page_size.get()) + 1)
            .cloned()
            .collect();
        let has_more = items.len() > usize::from(query.page_size.get());
        items.truncate(usize::from(query.page_size.get()));
        let next_before_generation = if has_more {
            items.last().map(|item| item.generation)
        } else {
            None
        };
        Ok(ChannelDiscoveryPage {
            items,
            next_before_generation,
        })
    }
    async fn reserve_model_discovery(
        &self,
        id: &ChannelId,
        revision: ChannelRevision,
    ) -> AdminStoreResult<u64> {
        let mut state = self.state.lock().expect("state");
        if !state
            .stored
            .as_ref()
            .is_some_and(|stored| stored.id == *id && stored.revision == revision)
        {
            return Err(discovery_conflict());
        }
        state.generation += 1;
        Ok(state.generation)
    }
    async fn save_model_discovery(&self, preview: &ChannelModelPreview) -> AdminStoreResult<()> {
        let mut state = self.state.lock().expect("state");
        if state.reject {
            return Err(unavailable("channel"));
        }
        if !state
            .stored
            .as_ref()
            .is_some_and(|stored| stored.id == preview.id && stored.revision == preview.revision)
            || state
                .discoveries
                .last()
                .is_some_and(|old| old.generation >= preview.generation)
        {
            return Err(discovery_conflict());
        }
        preview.validate().expect("valid snapshot");
        state.discoveries.push(preview.clone());
        Ok(())
    }
    async fn load_model_discovery(
        &self,
        id: &ChannelId,
    ) -> AdminStoreResult<Option<ChannelModelPreview>> {
        let state = self.state.lock().expect("state");
        if state.reject {
            return Err(unavailable("channel"));
        }
        if !state.stored.as_ref().is_some_and(|stored| stored.id == *id) {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "channel",
                "missing",
            ));
        }
        Ok(state.discoveries.last().cloned())
    }
    async fn list_channels(&self, _: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        Err(unavailable("unused list"))
    }
    async fn load_channel_for_edit(
        &self,
        id: &ChannelId,
    ) -> AdminStoreResult<Option<StoredChannel>> {
        Ok(self
            .state
            .lock()
            .expect("state")
            .stored
            .as_ref()
            .filter(|stored| stored.id == *id)
            .cloned())
    }
    async fn change_channel(
        &self,
        change: ChannelChange,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        let mut state = self.state.lock().expect("state");
        if state.reject {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Unavailable,
                "channel",
                "simulated store failure",
            ));
        }
        match change {
            ChannelChange::Create {
                id,
                provider,
                config,
                ..
            } => {
                state.stored = Some(StoredChannel {
                    id,
                    provider,
                    revision: rev(1),
                    config,
                })
            }
            ChannelChange::Update {
                id,
                expected_revision,
                replacement_config,
                ..
            } => {
                let stored = state
                    .stored
                    .as_mut()
                    .filter(|s| s.id == id && s.revision == expected_revision)
                    .ok_or_else(|| {
                        AdminStoreError::new(
                            AdminStoreErrorKind::StaleRevision,
                            "channel",
                            "stale update",
                        )
                    })?;
                stored.revision = rev(stored.revision.get() + 1);
                if let Some(config) = replacement_config {
                    stored.config = config;
                }
            }
            ChannelChange::Delete {
                id,
                expected_revision,
            } => {
                if !state
                    .stored
                    .as_ref()
                    .is_some_and(|s| s.id == id && s.revision == expected_revision)
                {
                    return Err(AdminStoreError::new(
                        AdminStoreErrorKind::StaleRevision,
                        "channel",
                        "stale delete",
                    ));
                }
                state.stored = None;
                state.discoveries.clear();
            }
        }
        state.commits += 1;
        Ok(Revision::new(state.commits).expect("revision"))
    }
}

struct TestProvider {
    kind: ProviderKind,
}

fn discovery_conflict() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::StaleRevision,
        "channel",
        "stale discovery",
    )
}

struct DiscoveryProvider {
    inner: TestProvider,
    store: Arc<MemoryChannels>,
    mode: std::sync::atomic::AtomicU8,
    calls: std::sync::atomic::AtomicUsize,
}

impl ChannelProviderAdmin for DiscoveryProvider {
    fn provider_kind(&self) -> &ProviderKind {
        self.inner.provider_kind()
    }
    fn prepare_config(
        &self,
        input: &ProviderDocument,
        current: Option<&ProviderChannelConfig>,
    ) -> Result<ProviderChannelConfig, ProviderAdminError> {
        self.inner.prepare_config(input, current)
    }
    fn public_config(
        &self,
        config: &ProviderChannelConfig,
    ) -> Result<ProviderDocument, ProviderAdminError> {
        self.inner.public_config(config)
    }
    fn discover_models<'a>(
        &'a self,
        _: &'a ProviderChannelConfig,
    ) -> BoxFuture<
        'a,
        Result<gateway_admin::model::channels::DiscoveredChannelModels, ProviderAdminError>,
    > {
        Box::pin(async move {
            use std::sync::atomic::Ordering;
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.mode.load(Ordering::SeqCst) {
                1 => return Err(ProviderAdminError::new(ProviderAdminErrorKind::BadGateway)),
                2 => {
                    self.store
                        .state
                        .lock()
                        .expect("state")
                        .stored
                        .as_mut()
                        .expect("stored")
                        .revision = rev(2)
                }
                _ => {}
            }
            Ok(gateway_admin::model::channels::DiscoveredChannelModels {
                configured: ["existing", "missing"]
                    .map(|id| gateway_core::routing::UpstreamModelId::new(id).expect("id"))
                    .into(),
                discovered: ["existing", "new-model"]
                    .map(|id| gateway_core::routing::UpstreamModelId::new(id).expect("id"))
                    .into(),
            })
        })
    }
}

#[tokio::test]
async fn discovery_persists_only_success_without_changing_config_and_rejects_stale_versions() {
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    let store = Arc::new(MemoryChannels::default());
    let provider = Arc::new(DiscoveryProvider {
        inner: TestProvider { kind: kind() },
        store: store.clone(),
        mode: AtomicU8::new(0),
        calls: AtomicUsize::new(0),
    });
    let services = AdminHarness::new()
        .channels(store.clone(), provider.clone())
        .build()
        .await;
    let service = services.channels();
    let created = service
        .create(
            &context(),
            NewChannel {
                provider: kind(),
                fields: fields(),
                config: document(json!({"secret":"private-token"})),
            },
        )
        .await
        .expect("create");
    assert!(
        service
            .last_model_discovery(&created.id)
            .await
            .expect("no discovery")
            .is_none()
    );
    let preview = service
        .discover_models(&created.id, rev(1))
        .await
        .expect("preview");
    assert_eq!(preview.revision, rev(1));
    assert_eq!(preview.added, ["new-model"]);
    assert_eq!(preview.missing, ["missing"]);
    assert_eq!(preview.unchanged, ["existing"]);
    assert_eq!(
        service
            .last_model_discovery(&created.id)
            .await
            .expect("restore"),
        Some(preview.clone())
    );
    assert_eq!(store.state.lock().expect("state").commits, 1);
    assert!(!format!("{preview:?}").contains("private-token"));
    assert_eq!(
        service
            .discover_models(&created.id, rev(2))
            .await
            .expect_err("stale before send")
            .kind(),
        AdminErrorKind::Conflict
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    provider.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        service
            .discover_models(&created.id, rev(1))
            .await
            .expect_err("failed query")
            .kind(),
        AdminErrorKind::BadGateway
    );
    assert_eq!(store.state.lock().expect("state").commits, 1);
    provider.mode.store(2, Ordering::SeqCst);
    assert_eq!(
        service
            .discover_models(&created.id, rev(1))
            .await
            .expect_err("stale during query")
            .kind(),
        AdminErrorKind::Conflict
    );
    assert_eq!(store.state.lock().expect("state").commits, 1);
    assert_eq!(
        service
            .last_model_discovery(&created.id)
            .await
            .expect("retained stale snapshot"),
        Some(preview.clone())
    );
    provider.mode.store(0, Ordering::SeqCst);
    store.state.lock().expect("state").reject = true;
    assert_eq!(
        service
            .discover_models(&created.id, rev(2))
            .await
            .expect_err("failed persistence")
            .kind(),
        AdminErrorKind::Unavailable
    );
    store.state.lock().expect("state").reject = false;
    assert_eq!(
        service
            .last_model_discovery(&created.id)
            .await
            .expect("old success not overwritten"),
        Some(preview.clone())
    );
    let newer = service
        .discover_models(&created.id, rev(2))
        .await
        .expect("next success");
    let calls = provider.calls.load(Ordering::SeqCst);
    let mut query = ChannelDiscoveryQuery {
        id: created.id,
        before_generation: None,
        page_size: gateway_admin::model::PageSize::new(1).expect("size"),
    };
    let page = service
        .model_discovery_history(query.clone())
        .await
        .expect("newest page");
    assert_eq!(page.items, vec![newer.clone()]);
    assert_eq!(page.next_before_generation, Some(newer.generation));
    query.before_generation = page.next_before_generation;
    let page = service
        .model_discovery_history(query.clone())
        .await
        .expect("older page");
    assert_eq!(page.items, vec![preview]);
    assert_eq!(page.next_before_generation, None);
    query.before_generation = Some(0);
    assert_eq!(
        service
            .model_discovery_history(query.clone())
            .await
            .expect_err("invalid before store")
            .kind(),
        AdminErrorKind::Invalid
    );
    query.before_generation = None;
    store.state.lock().expect("state").reject = true;
    assert_eq!(
        service
            .model_discovery_history(query)
            .await
            .expect_err("not empty on failure")
            .kind(),
        AdminErrorKind::Unavailable
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
    assert_eq!(store.state.lock().expect("state").commits, 1);
}
impl ChannelProviderAdmin for TestProvider {
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }
    fn prepare_config(
        &self,
        input: &ProviderDocument,
        current: Option<&ProviderChannelConfig>,
    ) -> Result<ProviderChannelConfig, ProviderAdminError> {
        let input = input.expose_to_provider().expose_to_provider();
        if input.contains_key("invalid") {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::Invalid));
        }
        let mut config = current
            .map(|c| c.expose_to_provider().clone())
            .unwrap_or_default();
        config.extend(input.clone());
        ProviderChannelConfig::new(config)
            .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Invalid))
    }
    fn public_config(
        &self,
        _: &ProviderChannelConfig,
    ) -> Result<ProviderDocument, ProviderAdminError> {
        Ok(document(json!({"hasSecret": true})))
    }
}

#[tokio::test]
async fn comparison_reads_saved_pairs_without_credentials_and_rejects_missing_or_failed_reads() {
    let base = ChannelModelPreview {
        id: ChannelId::new("chan_compare").expect("id"),
        revision: rev(1),
        generation: 9007199254740993,
        fetched_at: chrono::Utc::now(),
        added: vec!["new-model".to_owned()],
        missing: vec!["configured".to_owned()],
        unchanged: vec![],
    };
    let mut target = base.clone();
    target.generation += 1;
    let store = Arc::new(MemoryChannels::default());
    store.state.lock().expect("state").discoveries = vec![base.clone(), target.clone()];
    let services = AdminHarness::new()
        .channels(store.clone(), Arc::new(TestProvider { kind: kind() }))
        .build()
        .await;
    let mut query = ChannelDiscoveryComparisonQuery {
        id: base.id,
        base_generation: base.generation,
        target_generation: target.generation,
    };
    let comparison = services
        .channels()
        .compare_model_discoveries(query.clone())
        .await
        .expect("local comparison");
    assert!(comparison.appeared.is_empty() && comparison.disappeared.is_empty());
    assert_eq!(comparison.unchanged, ["new-model"]);
    query.target_generation += 1;
    assert_eq!(
        services
            .channels()
            .compare_model_discoveries(query.clone())
            .await
            .expect_err("missing record")
            .kind(),
        AdminErrorKind::NotFound
    );
    query.target_generation = query.base_generation;
    assert_eq!(
        services
            .channels()
            .compare_model_discoveries(query.clone())
            .await
            .expect_err("invalid order")
            .kind(),
        AdminErrorKind::Invalid
    );
    query.target_generation = target.generation;
    store.state.lock().expect("state").reject = true;
    assert_eq!(
        services
            .channels()
            .compare_model_discoveries(query)
            .await
            .expect_err("failure is not no changes")
            .kind(),
        AdminErrorKind::Unavailable
    );
    let state = store.state.lock().expect("state");
    assert_eq!(state.commits, 0);
    assert_eq!(state.generation, 0);
    assert_eq!(state.discoveries.len(), 2);
}

#[derive(Default)]
struct Publications(Mutex<Vec<ConfigRevision>>);
impl SnapshotControl for Publications {
    fn publish_committed(&self, revision: ConfigRevision) -> BoxFuture<'_, ()> {
        self.0.lock().expect("publications").push(revision);
        Box::pin(async {})
    }
}
fn document(value: Value) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(
        value.as_object().expect("object").clone(),
    ))
}
fn rev(n: u64) -> ChannelRevision {
    ChannelRevision::new(n).expect("revision")
}
fn kind() -> ProviderKind {
    ProviderKind::new("test_api").expect("provider")
}
fn fields() -> ChannelFields {
    ChannelFields {
        name: "A channel".to_owned(),
        note: None,
        enabled: true,
        preference: SourcePreference::default(),
        limits: RateLimits::default(),
        quota_scope_id: None,
    }
}
fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: "req_channel_test".to_owned(),
    }
}

#[tokio::test]
async fn channel_service_redacts_edits_preserves_omitted_credentials_and_fences_stale_writes() {
    let store = Arc::new(MemoryChannels::default());
    let publications = Arc::new(Publications::default());
    let services = AdminHarness::new()
        .channels(store.clone(), Arc::new(TestProvider { kind: kind() }))
        .snapshot(publications.clone())
        .build()
        .await;
    let service = services.channels();
    assert_eq!(service.providers(), [kind()]);
    let created = service
        .create(
            &context(),
            NewChannel {
                provider: kind(),
                fields: fields(),
                config: document(json!({"secret": "private-token"})),
            },
        )
        .await
        .expect("create");
    let connection = service.connection(&created.id).await.expect("connection");
    assert_eq!(connection.revision, rev(1));
    assert_eq!(
        connection.config.expose_to_provider().expose_to_provider(),
        &json!({"hasSecret": true})
            .as_object()
            .expect("object")
            .clone()
    );
    assert!(!format!("{connection:?}").contains("private-token"));
    let changed = service
        .update(
            &context(),
            UpdateChannel {
                id: created.id.clone(),
                expected_revision: rev(1),
                fields: fields(),
                config: Some(document(json!({"project": "new"}))),
            },
        )
        .await
        .expect("merge update");
    assert_eq!(changed.config_revision.get(), 2);
    let stored = store
        .load_channel_for_edit(&created.id)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(
        stored.config.expose_to_provider()["secret"],
        "private-token"
    );
    assert_eq!(stored.config.expose_to_provider()["project"], "new");
    assert_eq!(
        service
            .update(
                &context(),
                UpdateChannel {
                    id: created.id.clone(),
                    expected_revision: rev(1),
                    fields: fields(),
                    config: None
                }
            )
            .await
            .expect_err("stale")
            .kind(),
        AdminErrorKind::Conflict
    );
    assert_eq!(
        service
            .update(
                &context(),
                UpdateChannel {
                    id: created.id.clone(),
                    expected_revision: rev(2),
                    fields: fields(),
                    config: Some(document(json!({"invalid": true})))
                }
            )
            .await
            .expect_err("invalid provider config")
            .kind(),
        AdminErrorKind::Invalid
    );
    assert_eq!(
        service
            .delete(&context(), created.id.clone(), rev(1))
            .await
            .expect_err("stale delete")
            .kind(),
        AdminErrorKind::Conflict
    );
    store.state.lock().expect("state").reject = true;
    assert_eq!(
        service
            .delete(&context(), created.id.clone(), rev(2))
            .await
            .expect_err("store failure")
            .kind(),
        AdminErrorKind::Unavailable
    );
    assert_eq!(publications.0.lock().expect("publications").len(), 2);
    store.state.lock().expect("state").reject = false;
    service
        .delete(&context(), created.id.clone(), rev(2))
        .await
        .expect("delete");
    assert_eq!(
        service
            .connection(&created.id)
            .await
            .expect_err("missing")
            .kind(),
        AdminErrorKind::NotFound
    );
    assert_eq!(publications.0.lock().expect("publications").len(), 3);
    assert_eq!(store.state.lock().expect("state").commits, 3);
}

#[tokio::test]
async fn channel_service_rejects_unknown_adapters_and_invalid_fields_without_persisting() {
    let store = Arc::new(MemoryChannels::default());
    let services = AdminHarness::new()
        .channels(store.clone(), Arc::new(TestProvider { kind: kind() }))
        .build()
        .await;
    let service = services.channels();
    let mut invalid_fields = fields();
    invalid_fields.name.clear();
    for (provider, fields) in [
        (ProviderKind::new("unregistered").expect("kind"), fields()),
        (kind(), invalid_fields),
    ] {
        assert_eq!(
            service
                .create(
                    &context(),
                    NewChannel {
                        provider,
                        fields,
                        config: document(json!({"secret": "private-token"}))
                    }
                )
                .await
                .expect_err("invalid")
                .kind(),
            AdminErrorKind::Invalid
        );
    }
    assert_eq!(store.state.lock().expect("state").commits, 0);
}
