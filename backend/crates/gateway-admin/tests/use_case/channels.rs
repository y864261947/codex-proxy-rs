use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_admin::{
    model::{
        AdminErrorKind, MutationActor, MutationContext, Revision,
        channels::{
            ChannelChange, ChannelFields, ChannelListQuery, ChannelPage, NewChannel, UpdateChannel,
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
}

#[async_trait]
impl ChannelStore for MemoryChannels {
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
            }
        }
        state.commits += 1;
        Ok(Revision::new(state.commits).expect("revision"))
    }
}

struct TestProvider {
    kind: ProviderKind,
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
