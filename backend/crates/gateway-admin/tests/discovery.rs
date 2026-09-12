use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_admin::{
    discovery::ChannelDiscoveryTask,
    model::{MutationContext, Revision, channels::*, provider_credentials::ProviderDocument},
    ports::{
        channels::ChannelProviderAdmin,
        provider::{ProviderAdminError, ProviderAdminErrorKind},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ChannelStore},
    },
};
use gateway_core::{
    channel::{ChannelRevision, ProviderChannelConfig, StoredChannel},
    identity::{ChannelId, ProviderKind},
    lifecycle::CancellationToken,
    routing::UpstreamModelId,
    task::{ScheduledTask, WorkerCycleContext, WorkerId, WorkerKind, WorkerRunnable},
};

struct ScheduleStore {
    due: Mutex<bool>,
    claims: AtomicUsize,
    saved: Mutex<Vec<ChannelModelPreview>>,
    finished: Mutex<Vec<bool>>,
    reject_finish: bool,
}

#[async_trait]
impl ChannelStore for ScheduleStore {
    async fn claim_due_model_discovery(&self) -> AdminStoreResult<Option<ChannelDiscoveryClaim>> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        Ok(
            std::mem::take(&mut *self.due.lock().expect("due")).then(|| ChannelDiscoveryClaim {
                id: id(),
                revision: revision(),
                attempt: 1,
            }),
        )
    }
    async fn finish_scheduled_model_discovery(
        &self,
        claim: &ChannelDiscoveryClaim,
        succeeded: bool,
    ) -> AdminStoreResult<()> {
        assert_eq!(claim.id, id());
        assert_eq!(claim.attempt, 1);
        if self.reject_finish {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Unavailable,
                "channel",
                "private store failure",
            ));
        }
        self.finished.lock().expect("finished").push(succeeded);
        Ok(())
    }
    async fn load_channel_for_edit(
        &self,
        channel: &ChannelId,
    ) -> AdminStoreResult<Option<StoredChannel>> {
        assert_eq!(channel, &id());
        Ok(Some(StoredChannel {
            id: id(),
            provider: kind(),
            revision: revision(),
            config: config(),
        }))
    }
    async fn reserve_model_discovery(
        &self,
        channel: &ChannelId,
        version: ChannelRevision,
    ) -> AdminStoreResult<u64> {
        assert_eq!(channel, &id());
        assert_eq!(version, revision());
        Ok(2)
    }
    async fn save_model_discovery(&self, preview: &ChannelModelPreview) -> AdminStoreResult<()> {
        preview.validate().expect("valid preview");
        self.saved.lock().expect("saved").push(preview.clone());
        Ok(())
    }
    async fn load_discovery_pair(
        &self,
        _: ChannelDiscoveryComparisonQuery,
    ) -> AdminStoreResult<Option<ChannelDiscoveryPair>> {
        unreachable!()
    }
    async fn list_model_discoveries(
        &self,
        _: ChannelDiscoveryQuery,
    ) -> AdminStoreResult<ChannelDiscoveryPage> {
        unreachable!()
    }
    async fn load_model_discovery(
        &self,
        _: &ChannelId,
    ) -> AdminStoreResult<Option<ChannelModelPreview>> {
        unreachable!()
    }
    async fn list_channels(&self, _: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        unreachable!()
    }
    async fn change_channel(
        &self,
        _: ChannelChange,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        unreachable!()
    }
}

struct DiscoveryProvider {
    kind: ProviderKind,
    calls: AtomicUsize,
    mode: u8,
    entered: tokio::sync::Notify,
}

impl ChannelProviderAdmin for DiscoveryProvider {
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }
    fn prepare_config(
        &self,
        _: &ProviderDocument,
        _: Option<&ProviderChannelConfig>,
    ) -> Result<ProviderChannelConfig, ProviderAdminError> {
        unreachable!()
    }
    fn public_config(
        &self,
        _: &ProviderChannelConfig,
    ) -> Result<ProviderDocument, ProviderAdminError> {
        unreachable!()
    }
    fn discover_models<'a>(
        &'a self,
        _: &'a ProviderChannelConfig,
    ) -> BoxFuture<'a, Result<DiscoveredChannelModels, ProviderAdminError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            match self.mode {
                1 => Err(ProviderAdminError::new(ProviderAdminErrorKind::BadGateway)),
                2 => std::future::pending().await,
                _ => Ok(DiscoveredChannelModels {
                    configured: [UpstreamModelId::new("existing").expect("model")].into(),
                    discovered: [UpstreamModelId::new("new-model").expect("model")].into(),
                }),
            }
        })
    }
}

async fn fixture(
    due: bool,
    mode: u8,
    reject_finish: bool,
) -> (
    ChannelDiscoveryTask,
    Arc<ScheduleStore>,
    Arc<DiscoveryProvider>,
) {
    let store = Arc::new(ScheduleStore {
        due: Mutex::new(due),
        claims: AtomicUsize::new(0),
        saved: Mutex::new(Vec::new()),
        finished: Mutex::new(Vec::new()),
        reject_finish,
    });
    let provider = Arc::new(DiscoveryProvider {
        kind: kind(),
        calls: AtomicUsize::new(0),
        mode,
        entered: tokio::sync::Notify::new(),
    });
    let services = crate::use_case::AdminHarness::new()
        .channels(store.clone(), provider.clone())
        .build()
        .await;
    (
        ChannelDiscoveryTask::new(store.clone(), services),
        store,
        provider,
    )
}

fn id() -> ChannelId {
    ChannelId::new("chan_schedule").expect("id")
}
fn kind() -> ProviderKind {
    ProviderKind::new("openai_api").expect("kind")
}
fn revision() -> ChannelRevision {
    ChannelRevision::new(1).expect("revision")
}
fn config() -> ProviderChannelConfig {
    ProviderChannelConfig::new(serde_json::Map::from_iter([(
        "token".to_owned(),
        serde_json::Value::String("fixture-secret".to_owned()),
    )]))
    .expect("config")
}
fn context(cancellation: CancellationToken) -> WorkerCycleContext {
    WorkerCycleContext::new(
        WorkerId::try_new(WorkerKind::ChannelModelDiscovery, "channel-discovery").expect("worker"),
        None,
        cancellation,
    )
}

#[tokio::test]
async fn idle_and_pre_cancelled_cycles_never_query_the_provider() {
    let (task, store, provider) = fixture(false, 0, false).await;
    task.run_cycle(context(CancellationToken::new()))
        .await
        .expect("idle");
    assert_eq!(store.claims.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    task.run_cycle(context(cancellation))
        .await
        .expect("cancelled");
    assert_eq!(store.claims.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn scheduled_cycle_saves_one_preview_and_confirms_only_after_persistence() {
    let (task, store, provider) = fixture(true, 0, false).await;
    task.run_cycle(context(CancellationToken::new()))
        .await
        .expect("success");
    task.run_cycle(context(CancellationToken::new()))
        .await
        .expect("next idle cycle");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let saved = store.saved.lock().expect("saved");
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].added, vec!["new-model"]);
    assert_eq!(saved[0].missing, vec!["existing"]);
    assert_eq!(*store.finished.lock().expect("finished"), vec![true]);
}

#[tokio::test]
async fn upstream_failure_records_unconfirmed_success_without_a_new_preview() {
    let (task, store, provider) = fixture(true, 1, false).await;
    assert!(
        task.run_cycle(context(CancellationToken::new()))
            .await
            .is_err()
    );
    assert!(store.saved.lock().expect("saved").is_empty());
    assert_eq!(*store.finished.lock().expect("finished"), vec![false]);
    task.run_cycle(context(CancellationToken::new()))
        .await
        .expect("no immediate retry");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_during_query_leaves_attempt_unconfirmed_and_stops_work() {
    let (task, store, provider) = fixture(true, 2, false).await;
    let cancellation = CancellationToken::new();
    let (result, ()) = tokio::join!(task.run_cycle(context(cancellation.clone())), async {
        provider.entered.notified().await;
        cancellation.cancel();
    });
    result.expect("cancelled");
    assert!(store.saved.lock().expect("saved").is_empty());
    assert!(store.finished.lock().expect("finished").is_empty());
}

#[tokio::test]
async fn completion_failure_does_not_erase_success_or_expose_store_error() {
    let (task, store, _) = fixture(true, 0, true).await;
    let error = task
        .run_cycle(context(CancellationToken::new()))
        .await
        .expect_err("status unknown");
    assert!(!error.as_safe_str().contains("private"));
    assert_eq!(store.saved.lock().expect("saved").len(), 1);
    assert!(store.finished.lock().expect("finished").is_empty());
}

#[tokio::test(start_paused = true)]
async fn deadline_leaves_attempt_unconfirmed_without_immediate_retry() {
    let (task, store, provider) = fixture(true, 2, false).await;
    let started = tokio::time::Instant::now();
    assert!(
        task.run_cycle(context(CancellationToken::new()))
            .await
            .is_err()
    );
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(30));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(store.saved.lock().expect("saved").is_empty());
    assert!(store.finished.lock().expect("finished").is_empty());
    task.run_cycle(context(CancellationToken::new()))
        .await
        .expect("no catchup");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn discovery_contributes_one_host_supervised_leased_worker() {
    let (task, _, _) = fixture(false, 0, false).await;
    let contribution = task.contribution().expect("contribution");
    contribution.validate().expect("valid registration");
    assert_eq!(contribution.kind(), WorkerKind::ChannelModelDiscovery);
    let gateway_core::task::WorkerContribution::Registration(registration) = contribution else {
        panic!("registration")
    };
    assert!(matches!(
        registration.runnable,
        WorkerRunnable::Scheduled { lease: Some(_), .. }
    ));
}
