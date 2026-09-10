use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{executor::block_on, future::BoxFuture};
use gateway_core::account::{AccountSelectionPolicy, ProviderAccountId, RotationStrategy};
use gateway_core::engine::admission::{
    ClientAdmissionDecision, ClientAdmissionError, ClientAdmissionPort, ClientAdmissionRecovery,
    ClientAdmissionRequest, ClientAdmissionRestoreResult,
};
use gateway_core::engine::continuation::{
    NativeContinuationPin, NativeContinuationPort, NativeContinuationStoreError, PreviousResponseId,
};
use gateway_core::engine::execution::{
    ClientApiKeyUsageSink, ClientTransport, DefaultExecutionService, ExecutionRequestMetadata,
    ExecutionService, ProviderCircuitDecision, ProviderCircuitError, ProviderCircuitPort,
    ProviderCircuitScope, StartExecution, StartProviderExecution, provider_failure_affects_circuit,
};
use gateway_core::engine::probe::{AccountProbe, AccountProbeErrorSource, AccountProbeRequest};
use gateway_core::engine::provider::{
    Provider, ProviderCallMetadata, ProviderCatalogGeneration, ProviderModelCapabilities,
    ProviderRegistry, ProviderRequest, ProviderRequestObservation, ProviderStream,
};
use gateway_core::engine::{
    AttemptContext, AttemptRecord, ExecutionStore, IntermediateFailure, ModelRequestFinalization,
    ModelRequestId, NewModelRequest, ProbeFailure, RecoveryReport,
};
use gateway_core::error::{
    ClientVisibleUpstreamResponse, GatewayErrorKind, ProviderError, ProviderErrorKind, StoreError,
    StoreErrorKind,
};
use gateway_core::operation::{
    GenerateRequest, ImageRequest, ImageRequestKind, Operation, OperationKind, ProtocolPayload,
    RawJsonPayload,
};
use gateway_core::policy::{ClientApiKeyId, ClientPolicy, PlaintextClientApiKey, RateLimits};
use gateway_core::routing::{
    ClientRoutingScope, ConfigRevision, FrozenAccountScope, ModelCapabilities, ProviderKind,
    ProviderModel, PublicModelId, RuntimeAccount, RuntimeAccountDirectory, RuntimeSnapshot,
    UpstreamModelId,
};
use gateway_core::runtime::RuntimeSnapshotHandle;
use gateway_core::upstream::{UpstreamSendState, UpstreamTransport};
use serde_json::json;

#[test]
fn only_provider_attributable_failures_should_affect_circuit() {
    assert!(provider_failure_affects_circuit(ProviderErrorKind::Timeout));
    assert!(provider_failure_affects_circuit(
        ProviderErrorKind::Transport
    ));
    assert!(!provider_failure_affects_circuit(
        ProviderErrorKind::RateLimited
    ));
    assert!(!provider_failure_affects_circuit(
        ProviderErrorKind::InvalidRequest
    ));
    assert!(!provider_failure_affects_circuit(
        ProviderErrorKind::ContinuationRecoveryRequired
    ));
}

#[test]
fn account_probe_should_not_write_to_the_persistent_execution_store() {
    let store = Arc::new(TrackingExecutionStore::default());
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(probe_snapshot()),
        store.clone(),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );

    let error = block_on(service.probe(AccountProbeRequest {
        account_id: ProviderAccountId::new("acct_probe").expect("account ID"),
        provider_kind: ProviderKind::new("openai").expect("provider kind"),
        upstream_model: UpstreamModelId::new("gpt-probe").expect("model ID"),
        operation: probe_operation(),
    }))
    .expect_err("empty Provider registry should stop the probe after it starts");

    assert_eq!(error.kind(), GatewayErrorKind::NoAvailableProvider);
    assert_eq!(error.source(), AccountProbeErrorSource::Gateway);
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
    assert_eq!(
        service
            .traffic_monitor()
            .snapshot()
            .ingress_requests_last_minute,
        0
    );
    assert_eq!(error.send_state(), None);
    assert!(!store.touched.load(Ordering::SeqCst));
}

#[test]
fn probe_failures_should_be_observable_without_a_model_request_row() {
    let store = Arc::new(TrackingExecutionStore::default());
    let providers = ProviderRegistry::new([Arc::new(FailingProvider) as Arc<dyn Provider>])
        .expect("provider registry");
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(probe_snapshot()),
        store.clone(),
        providers,
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );

    let error = block_on(service.probe(AccountProbeRequest {
        account_id: ProviderAccountId::new("acct_probe").expect("account ID"),
        provider_kind: ProviderKind::new("openai").expect("provider kind"),
        upstream_model: UpstreamModelId::new("gpt-probe").expect("model ID"),
        operation: probe_operation(),
    }))
    .expect_err("the provider rejects every probe");

    assert_eq!(error.kind(), GatewayErrorKind::UpstreamUnavailable);
    assert_eq!(error.source(), AccountProbeErrorSource::Upstream);
    assert_eq!(error.send_state(), Some(UpstreamSendState::NotSent));
    let upstream = error
        .upstream_response()
        .expect("probe must preserve its request-local upstream response");
    assert_eq!(upstream.status(), 502);
    assert_eq!(
        upstream.content_type(),
        Some(b"application/json".as_slice())
    );
    assert_eq!(
        upstream.body(),
        &Bytes::from_static(br#"{"error":{"message":"source upstream failure"}}"#),
    );
    assert!(!format!("{error:?}").contains("source upstream failure"));
    assert!(!store.touched.load(Ordering::SeqCst));
    assert_eq!(store.probe_failures(), vec!["transport".to_owned()]);
}

#[test]
fn provider_local_probe_failure_should_remain_distinct_from_upstream() {
    let store = Arc::new(TrackingExecutionStore::default());
    let providers = ProviderRegistry::new([Arc::new(LocalFailingProvider) as Arc<dyn Provider>])
        .expect("provider registry");
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(probe_snapshot()),
        store,
        providers,
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );

    let error = block_on(service.probe(AccountProbeRequest {
        account_id: ProviderAccountId::new("acct_probe").expect("account ID"),
        provider_kind: ProviderKind::new("openai").expect("provider kind"),
        upstream_model: UpstreamModelId::new("gpt-probe").expect("model ID"),
        operation: probe_operation(),
    }))
    .expect_err("the Provider rejects the probe before sending it");

    assert_eq!(error.source(), AccountProbeErrorSource::Provider);
    assert_eq!(error.send_state(), Some(UpstreamSendState::NotSent));
    assert!(error.upstream_response().is_none());
}

#[test]
fn probe_observation_store_failure_preserves_the_provider_error() {
    let store = Arc::new(TrackingExecutionStore {
        fail_probe_observation: AtomicBool::new(true),
        ..TrackingExecutionStore::default()
    });
    let providers = ProviderRegistry::new([Arc::new(FailingProvider) as Arc<dyn Provider>])
        .expect("provider registry");
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(probe_snapshot()),
        store.clone(),
        providers,
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );

    let error = block_on(service.probe(AccountProbeRequest {
        account_id: ProviderAccountId::new("acct_probe").expect("account ID"),
        provider_kind: ProviderKind::new("openai").expect("provider kind"),
        upstream_model: UpstreamModelId::new("gpt-probe").expect("model ID"),
        operation: probe_operation(),
    }))
    .expect_err("the provider error must survive observation failure");

    assert_eq!(error.kind(), GatewayErrorKind::UpstreamUnavailable);
    assert!(!store.touched.load(Ordering::SeqCst));
    assert!(store.probe_failures().is_empty());
}

struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        _request: ProviderRequest,
        _context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(
            ProviderError::new(ProviderErrorKind::Transport, UpstreamSendState::NotSent)
                .with_client_visible_upstream_response(ClientVisibleUpstreamResponse::new(
                    502,
                    Some(b"application/json".to_vec()),
                    Bytes::from_static(br#"{"error":{"message":"source upstream failure"}}"#),
                )),
        )
    }
}

struct LocalFailingProvider;

#[async_trait]
impl Provider for LocalFailingProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            UpstreamSendState::NotSent,
        ))
    }
}

struct ColdFailingProvider {
    requested_model: Option<PublicModelId>,
}

#[async_trait]
impl Provider for ColdFailingProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }

    fn request_observation(&self, _: &Operation, _: &ClientApiKeyId) -> ProviderRequestObservation {
        ProviderRequestObservation {
            requested_model: self.requested_model.clone(),
            ..Default::default()
        }
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        request: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let metadata = ProviderCallMetadata::for_provider_endpoint(
            request.candidate().provider().clone(),
            ProviderAccountId::new("acct_usage").expect("account"),
            UpstreamTransport::new("http_json").expect("transport"),
        );
        Ok(ProviderStream::new(
            metadata,
            futures::stream::once(async {
                Err(ProviderError::new(
                    ProviderErrorKind::Transport,
                    UpstreamSendState::NotSent,
                ))
            }),
            (),
        ))
    }
}

#[test]
fn successful_authentication_should_record_client_key_usage() {
    let usage = Arc::new(RecordingClientApiKeyUsage::default());
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(client_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        usage.clone(),
    );

    service
        .authenticate("sk_usage_test")
        .expect("successful authentication");

    assert_eq!(usage.recorded(), vec!["key_usage_test".to_owned()]);
}

#[test]
fn provider_endpoint_should_persist_its_real_v1_endpoint() {
    assert_provider_endpoint_observation(None);
    assert_provider_endpoint_observation(Some("gpt-image-2"));
}

fn assert_provider_endpoint_observation(model: Option<&str>) {
    let store = Arc::new(TrackingExecutionStore::default());
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(client_snapshot()),
        store.clone(),
        ProviderRegistry::new([Arc::new(ColdFailingProvider {
            requested_model: model.map(|model| PublicModelId::new(model).expect("model")),
        }) as Arc<dyn Provider>])
        .expect("provider registry"),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_usage_test")
        .expect("authenticated client");
    let operation = Operation::GenerateImage(ImageRequest::from_raw_json(
        ImageRequestKind::Generation,
        RawJsonPayload::new(
            "openai",
            Bytes::from_static(br#"{"model":"gpt-image-2","prompt":"hello"}"#),
        )
        .expect("image payload"),
    ));

    let mut started = block_on(service.start_provider_endpoint(StartProviderExecution {
        client,
        provider: ProviderKind::new("openai").expect("provider"),
        operation,
        metadata: ExecutionRequestMetadata {
            protocol: "openai".to_owned(),
            endpoint: "/v1/images/generations".to_owned(),
            transport: ClientTransport::HttpJson,
            stream: false,
            client_ip: None,
            user_agent: None,
            previous_response_id: None,
        },
    }))
    .expect("provider endpoint request should start without a text catalog entry");

    assert_eq!(service.traffic_monitor().snapshot().executing_requests, 1);

    let error = block_on(started.session.collect_uncommitted())
        .expect_err("the cold provider stops execution after persistence");
    assert_eq!(
        store.model_request_endpoints(),
        vec!["/v1/images/generations".to_owned()],
        "execution error: {error:?}"
    );
    assert_eq!(store.requested_models(), vec![model.map(str::to_owned)]);
    let upstream_models = store.upstream_models();
    assert!(!upstream_models.is_empty());
    assert!(upstream_models.iter().all(Option::is_none));
    assert!(started.session.is_finalized());
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

#[test]
fn circuit_store_failure_should_fail_open_during_request_start() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(FailingDecisionCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_start_test")
        .expect("authenticated client");

    let started = block_on(service.start(StartExecution {
        client,
        public_model: PublicModelId::new("gpt-start").expect("public model"),
        operation: start_operation(),
        metadata: ExecutionRequestMetadata {
            protocol: "openai".to_owned(),
            endpoint: "/v1/responses".to_owned(),
            transport: ClientTransport::HttpJson,
            stream: false,
            client_ip: None,
            user_agent: None,
            previous_response_id: None,
        },
    }))
    .expect("recoverable circuit state must not reject the request");

    assert!(!started.session.is_finalized());
    assert_eq!(service.traffic_monitor().snapshot().executing_requests, 1);
    assert_eq!(service.traffic_monitor().snapshot().preparing_requests, 0);
    drop(started);
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

#[test]
fn slow_circuit_store_should_time_out_and_fail_open_during_request_start() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(PendingDecisionCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_start_test")
        .expect("authenticated client");
    let started_at = Instant::now();

    let started = block_on(service.start(StartExecution {
        client,
        public_model: PublicModelId::new("gpt-start").expect("public model"),
        operation: start_operation(),
        metadata: ExecutionRequestMetadata {
            protocol: "openai".to_owned(),
            endpoint: "/v1/responses".to_owned(),
            transport: ClientTransport::HttpJson,
            stream: false,
            client_ip: None,
            user_agent: None,
            previous_response_id: None,
        },
    }))
    .expect("slow recoverable circuit state must not reject the request");

    assert!(started_at.elapsed() < Duration::from_secs(2));
    assert!(!started.session.is_finalized());
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 1);
    block_on(started.session.detach_finalize());
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

#[test]
fn cancelling_request_preparation_releases_realtime_concurrency() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(PendingDecisionCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service.authenticate("sk_start_test").expect("client");
    block_on(async {
        let mut request = service.start(StartExecution {
            client,
            public_model: PublicModelId::new("gpt-start").unwrap(),
            operation: start_operation(),
            metadata: ExecutionRequestMetadata {
                protocol: "openai".to_owned(),
                endpoint: "/v1/responses".to_owned(),
                transport: ClientTransport::HttpSse,
                stream: true,
                client_ip: None,
                user_agent: None,
                previous_response_id: None,
            },
        });
        assert!(futures::poll!(&mut request).is_pending());
        assert_eq!(service.traffic_monitor().snapshot().preparing_requests, 1);
        drop(request);
        assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
    });
}

#[test]
fn known_catalog_should_reject_a_model_that_the_provider_did_not_publish() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_start_test")
        .expect("authenticated client");
    let model = "gpt-future-not-in-catalog";

    let result = block_on(service.start(StartExecution {
        client,
        public_model: PublicModelId::from_client_wire(model).expect("client model"),
        operation: start_operation_for_model(model),
        metadata: ExecutionRequestMetadata {
            protocol: "openai".to_owned(),
            endpoint: "/v1/responses".to_owned(),
            transport: ClientTransport::HttpJson,
            stream: false,
            client_ip: None,
            user_agent: None,
            previous_response_id: None,
        },
    }));
    let Err(error) = result else {
        panic!("a known provider catalog must be authoritative for model availability");
    };

    assert_eq!(error.kind(), GatewayErrorKind::NoAvailableProvider);
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

#[test]
fn continuation_owned_by_another_client_api_key_should_fail_closed() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(RejectedContinuation::OwnershipMismatch),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_start_test")
        .expect("authenticated client");

    let result = block_on(service.start(StartExecution {
        client,
        public_model: PublicModelId::new("gpt-start").expect("public model"),
        operation: start_operation(),
        metadata: execution_metadata_with_continuation(),
    }));
    let Err(error) = result else {
        panic!("cross-client continuation must be rejected");
    };

    assert_eq!(error.kind(), GatewayErrorKind::PolicyDenied);
}

#[test]
fn invalid_continuation_record_should_not_be_forwarded_as_an_external_handle() {
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(start_snapshot()),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(RejectedContinuation::InvalidData),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service
        .authenticate("sk_start_test")
        .expect("authenticated client");

    let result = block_on(service.start(StartExecution {
        client,
        public_model: PublicModelId::new("gpt-start").expect("public model"),
        operation: start_operation(),
        metadata: execution_metadata_with_continuation(),
    }));
    let Err(error) = result else {
        panic!("invalid continuation state must be rejected");
    };

    assert_eq!(error.kind(), GatewayErrorKind::Internal);
}

fn execution_metadata_with_continuation() -> ExecutionRequestMetadata {
    ExecutionRequestMetadata {
        protocol: "openai".to_owned(),
        endpoint: "/v1/responses".to_owned(),
        transport: ClientTransport::HttpJson,
        stream: false,
        client_ip: None,
        user_agent: None,
        previous_response_id: Some(PreviousResponseId::new("response-private")),
    }
}

#[derive(Default)]
struct RecordingClientApiKeyUsage {
    ids: Mutex<Vec<String>>,
}

impl RecordingClientApiKeyUsage {
    fn recorded(&self) -> Vec<String> {
        self.ids.lock().expect("recorded API Key IDs").clone()
    }
}

impl ClientApiKeyUsageSink for RecordingClientApiKeyUsage {
    fn record_used(&self, key_id: &ClientApiKeyId) {
        self.ids
            .lock()
            .expect("recorded API Key IDs")
            .push(key_id.as_str().to_owned());
    }
}

#[derive(Default)]
struct TrackingExecutionStore {
    touched: AtomicBool,
    probe_failures: Mutex<Vec<String>>,
    model_request_endpoints: Mutex<Vec<String>>,
    requested_models: Mutex<Vec<Option<String>>>,
    upstream_models: Mutex<Vec<Option<String>>>,
    fail_probe_observation: AtomicBool,
}

impl TrackingExecutionStore {
    fn touch(&self) {
        self.touched.store(true, Ordering::SeqCst);
    }

    fn probe_failures(&self) -> Vec<String> {
        self.probe_failures
            .lock()
            .expect("probe failures lock")
            .clone()
    }

    fn model_request_endpoints(&self) -> Vec<String> {
        self.model_request_endpoints
            .lock()
            .expect("model request endpoints lock")
            .clone()
    }

    fn requested_models(&self) -> Vec<Option<String>> {
        self.requested_models
            .lock()
            .expect("requested models lock")
            .clone()
    }

    fn upstream_models(&self) -> Vec<Option<String>> {
        self.upstream_models
            .lock()
            .expect("upstream models lock")
            .clone()
    }
}

#[async_trait]
impl ExecutionStore for TrackingExecutionStore {
    async fn create_model_request(&self, request: NewModelRequest) -> Result<(), StoreError> {
        self.touch();
        self.requested_models
            .lock()
            .expect("requested models lock")
            .push(
                request
                    .requested_model
                    .as_ref()
                    .map(|model| model.as_str().to_owned()),
            );
        self.model_request_endpoints
            .lock()
            .expect("model request endpoints lock")
            .push(request.endpoint);
        Ok(())
    }

    async fn record_attempt(&self, attempt: AttemptRecord) -> Result<(), StoreError> {
        self.touch();
        self.upstream_models
            .lock()
            .expect("upstream models lock")
            .push(
                attempt
                    .upstream_model_id
                    .as_ref()
                    .map(|model| model.as_str().to_owned()),
            );
        Ok(())
    }

    async fn mark_send_state(
        &self,
        _: &ModelRequestId,
        _: UpstreamSendState,
    ) -> Result<(), StoreError> {
        self.touch();
        Ok(())
    }

    async fn mark_downstream_committed(
        &self,
        _: &ModelRequestId,
        _: SystemTime,
        _: Option<u16>,
    ) -> Result<(), StoreError> {
        self.touch();
        Ok(())
    }

    async fn record_client_status(&self, _: &ModelRequestId, _: u16) -> Result<(), StoreError> {
        self.touch();
        Ok(())
    }

    async fn record_intermediate_failure(&self, _: IntermediateFailure) -> Result<(), StoreError> {
        self.touch();
        Ok(())
    }

    async fn record_probe_failure(&self, failure: ProbeFailure) -> Result<(), StoreError> {
        if self.fail_probe_observation.load(Ordering::SeqCst) {
            return Err(StoreError::new(StoreErrorKind::Unavailable));
        }
        self.probe_failures
            .lock()
            .expect("probe failures lock")
            .push(failure.error.kind().as_str().to_owned());
        Ok(())
    }

    async fn finalize_model_request(&self, _: ModelRequestFinalization) -> Result<(), StoreError> {
        self.touch();
        Ok(())
    }

    async fn recover_expired(&self, _: SystemTime) -> Result<RecoveryReport, StoreError> {
        self.touch();
        Ok(RecoveryReport::default())
    }
}

struct UnusedAdmissions;

struct RejectingAdmissions {
    reason: gateway_core::engine::admission::ClientAdmissionRejection,
    requests: Mutex<Vec<ClientAdmissionRequest>>,
}

impl RejectingAdmissions {
    fn new(reason: gateway_core::engine::admission::ClientAdmissionRejection) -> Self {
        Self {
            reason,
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl ClientAdmissionPort for RejectingAdmissions {
    fn admit(
        &self,
        request: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        assert!(
            request
                .scopes
                .iter()
                .any(|scope| scope.id == gateway_core::policy::AdmissionScopeId::Global)
        );
        self.requests.lock().expect("admissions").push(request);
        Box::pin(async { Ok(ClientAdmissionDecision::Rejected(self.reason)) })
    }

    fn release<'a>(
        &'a self,
        _: &'a [gateway_core::policy::AdmissionScopeId],
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async { panic!("rejected admission cannot own a lease") })
    }

    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { unreachable!("startup recovery not used") })
    }
}

#[test]
fn global_capacity_rejections_are_distinct_from_downstream_limits_and_do_not_start_execution() {
    use gateway_core::engine::admission::ClientAdmissionRejection;
    for (reason, expected) in [
        (
            ClientAdmissionRejection::GlobalConcurrencyLimited,
            GatewayErrorKind::NoAvailableProvider,
        ),
        (
            ClientAdmissionRejection::GlobalRateLimited,
            GatewayErrorKind::NoAvailableProvider,
        ),
        (
            ClientAdmissionRejection::ConcurrencyLimited,
            GatewayErrorKind::RateLimited,
        ),
        (
            ClientAdmissionRejection::RateLimited,
            GatewayErrorKind::RateLimited,
        ),
    ] {
        let store = Arc::new(TrackingExecutionStore::default());
        let service = DefaultExecutionService::new(
            RuntimeSnapshotHandle::new(start_snapshot()),
            store.clone(),
            ProviderRegistry::default(),
            (
                Arc::new(RejectingAdmissions::new(reason)),
                Arc::new(AllowedSourceAdmissions),
            ),
            Arc::new(UnusedCircuits),
            Arc::new(UnusedContinuation),
            Arc::new(RecordingClientApiKeyUsage::default()),
        );
        let client = service.authenticate("sk_start_test").expect("client");
        let error = block_on(service.start(StartExecution {
            client,
            public_model: PublicModelId::new("gpt-start").expect("model"),
            operation: start_operation(),
            metadata: access_test_metadata(),
        }))
        .err()
        .expect("admission rejected");
        assert_eq!(error.kind(), expected);
        assert!(!store.touched.load(Ordering::SeqCst));
        assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
    }
}

impl ClientAdmissionPort for UnusedAdmissions {
    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionDecision::Granted) })
    }

    fn release<'a>(
        &'a self,
        _: &'a [gateway_core::policy::AdmissionScopeId],
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async { Ok(true) })
    }

    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionRestoreResult::default()) })
    }
}

struct UnusedCircuits;

impl ProviderCircuitPort for UnusedCircuits {
    fn decision<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
        Box::pin(async { Ok(ProviderCircuitDecision::Allow) })
    }

    fn observe_failure<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }

    fn observe_success<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }
}

struct FailingDecisionCircuits;

impl ProviderCircuitPort for FailingDecisionCircuits {
    fn decision<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
        Box::pin(async { Err(ProviderCircuitError) })
    }

    fn observe_failure<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }

    fn observe_success<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }
}

struct PendingDecisionCircuits;

impl ProviderCircuitPort for PendingDecisionCircuits {
    fn decision<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
        Box::pin(futures::future::pending())
    }

    fn observe_failure<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }

    fn observe_success<'a>(
        &'a self,
        _: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async { Ok(()) })
    }
}

struct UnusedContinuation;

impl NativeContinuationPort for UnusedContinuation {
    fn resolve<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async { Ok(None) })
    }

    fn record<'a>(
        &'a self,
        _: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

enum RejectedContinuation {
    OwnershipMismatch,
    InvalidData,
}

impl NativeContinuationPort for RejectedContinuation {
    fn resolve<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async move {
            Err(match self {
                Self::OwnershipMismatch => NativeContinuationStoreError::ownership_mismatch(),
                Self::InvalidData => NativeContinuationStoreError::invalid_data("invalid record"),
            })
        })
    }

    fn record<'a>(
        &'a self,
        _: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

fn account_scope(provider: &ProviderKind, account_id: &str) -> Arc<FrozenAccountScope> {
    Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
            ProviderAccountId::new(account_id).expect("account ID"),
            RuntimeAccount::new(provider.clone(), BTreeSet::new()),
        )]))),
        ClientRoutingScope::all_accounts(),
    ))
}

fn probe_snapshot() -> RuntimeSnapshot {
    let provider = ProviderKind::new("openai").expect("provider kind");
    let directory = Arc::clone(account_scope(&provider, "acct_probe").directory());
    let capabilities =
        ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16_000));
    RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("config revision"),
        AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            std::num::NonZeroU32::new(1).expect("concurrency"),
            Duration::from_millis(1),
        ),
        vec![provider.clone()],
        vec![ProviderModel::new(
            provider,
            UpstreamModelId::new("gpt-probe").expect("model ID"),
            capabilities,
        )],
        Vec::new(),
    )
    .expect("probe snapshot")
    .with_account_directory(directory)
}

#[test]
fn fixed_account_probe_rejects_missing_accounts_and_provider_mismatch_before_dispatch() {
    let store = Arc::new(TrackingExecutionStore::default());
    let gate = Arc::new(LegacySourceGate::default());
    let provider = Arc::new(HealthProvider::default());
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(probe_snapshot()),
        store.clone(),
        ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).expect("registry"),
        (Arc::new(UnusedAdmissions), gate.clone()),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    for (account, provider_kind) in [("acct_missing", "openai"), ("acct_probe", "xai")] {
        let error = block_on(service.probe(AccountProbeRequest {
            account_id: ProviderAccountId::new(account).expect("account"),
            provider_kind: ProviderKind::new(provider_kind).expect("provider"),
            upstream_model: UpstreamModelId::new("gpt-probe").expect("model"),
            operation: probe_operation(),
        }))
        .expect_err("invalid diagnostic scope");
        assert_eq!(error.kind(), GatewayErrorKind::NoAvailableProvider);
        assert_eq!(error.source(), AccountProbeErrorSource::Gateway);
    }
    assert!(provider.calls.lock().expect("calls").is_empty());
    assert!(gate.requests.lock().expect("requests").is_empty());
    assert!(!store.touched.load(Ordering::SeqCst));
}

fn client_snapshot() -> RuntimeSnapshot {
    let provider = ProviderKind::new("openai").expect("provider kind");
    RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("config revision"),
        AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            std::num::NonZeroU32::new(1).expect("concurrency"),
            Duration::from_millis(1),
        ),
        vec![provider.clone()],
        Vec::new(),
        vec![ClientPolicy::new(
            ClientApiKeyId::new("key_usage_test").expect("client API key ID"),
            PlaintextClientApiKey::new("sk_usage_test").expect("plaintext client API key"),
            account_scope(&provider, "acct_usage"),
            true,
            RateLimits::unlimited(),
        )],
    )
    .expect("client snapshot")
}

fn start_snapshot() -> RuntimeSnapshot {
    start_snapshot_with_access_group(None)
}

fn start_snapshot_with_access_group(
    group: Option<gateway_core::policy::AccessGroupPolicy>,
) -> RuntimeSnapshot {
    start_snapshot_with_limits(group, RateLimits::unlimited())
}

fn start_snapshot_with_limits(
    group: Option<gateway_core::policy::AccessGroupPolicy>,
    global_limits: RateLimits,
) -> RuntimeSnapshot {
    start_snapshot_with_legacy_pools(group, global_limits, BTreeSet::new())
}

fn start_snapshot_with_legacy_pools(
    group: Option<gateway_core::policy::AccessGroupPolicy>,
    global_limits: RateLimits,
    legacy_pools: BTreeSet<gateway_core::routing::AccountGroupId>,
) -> RuntimeSnapshot {
    use gateway_core::routing::{
        RoutingGroupSnapshot,
        source::{SourceId, SourcePolicy, SourcePreference},
    };
    let provider = ProviderKind::new("openai").expect("provider kind");
    let pools = group
        .as_ref()
        .map(|group| group.pool_group_ids.clone())
        .unwrap_or(legacy_pools);
    let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
        ProviderAccountId::new("acct_start").expect("account"),
        RuntimeAccount::new(provider.clone(), pools.clone()),
    )])));
    let scope = match &group {
        None => ClientRoutingScope::all_accounts(),
        Some(_) if pools.is_empty() => ClientRoutingScope::no_accounts(),
        Some(_) => ClientRoutingScope::restricted(
            pools
                .iter()
                .map(|id| RoutingGroupSnapshot::new(id.clone(), "Test pool".to_owned()))
                .collect(),
            pools.clone(),
            BTreeSet::from([provider.clone()]),
        )
        .expect("restricted pool scope"),
    };
    let source_policies = pools
        .into_iter()
        .map(|id| {
            SourcePolicy::new(
                SourceId::AccountPool(id),
                true,
                SourcePreference::default(),
                RateLimits::unlimited(),
                None,
            )
            .expect("source")
            .with_name("Test pool".to_owned())
            .expect("name")
        })
        .collect();
    let capabilities =
        ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16_000));
    RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("config revision"),
        AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            std::num::NonZeroU32::new(1).expect("concurrency"),
            Duration::from_millis(1),
        ),
        vec![provider.clone()],
        vec![ProviderModel::new(
            provider,
            UpstreamModelId::new("gpt-start").expect("model ID"),
            capabilities,
        )],
        vec![
            ClientPolicy::new(
                ClientApiKeyId::new("key_start_test").expect("client API key ID"),
                PlaintextClientApiKey::new("sk_start_test").expect("plaintext client API key"),
                Arc::new(FrozenAccountScope::new(Arc::clone(&directory), scope)),
                true,
                RateLimits::unlimited(),
            )
            .with_access_group(group)
            .with_global_limits(global_limits),
        ],
    )
    .expect("start snapshot")
    .with_account_directory(directory)
    .with_source_policies(source_policies)
    .expect("source policies")
}

#[test]
fn access_group_authorizes_public_alias_before_mapping_and_filters_model_endpoints() {
    use gateway_core::policy::{AccessGroupId, AccessGroupPolicy};
    let snapshot = start_snapshot_with_access_group(Some(AccessGroupPolicy {
        id: AccessGroupId::new("access_alias").expect("group"),
        enabled: true,
        limits: RateLimits::unlimited(),
        allowed_models: BTreeSet::from(["public-alias".to_owned()]),
        channel_ids: std::collections::BTreeSet::new(),
        pool_group_ids: BTreeSet::from([gateway_core::routing::AccountGroupId::new(
            "grp_11111111111111111111111111111111",
        )
        .expect("pool")]),
    }))
    .with_model_mappings(BTreeMap::from([(
        "public-alias".to_owned(),
        "gpt-start".to_owned(),
    )]));
    let store = Arc::new(TrackingExecutionStore::default());
    let service = DefaultExecutionService::new(
        RuntimeSnapshotHandle::new(snapshot),
        store.clone(),
        ProviderRegistry::default(),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let client = service.authenticate("sk_start_test").expect("client");
    assert_eq!(
        service
            .public_models(&client)
            .iter()
            .map(|model| model.as_str())
            .collect::<Vec<_>>(),
        ["public-alias"]
    );
    assert!(
        !service.contains_public_model(&client, &PublicModelId::new("gpt-start").expect("model"))
    );
    assert!(
        service.contains_public_model(&client, &PublicModelId::new("public-alias").expect("model"))
    );
    assert!(
        service
            .public_model_profiles(&client)
            .iter()
            .all(|profile| profile.model().as_str() == "public-alias")
    );
    for model in ["gpt-start", "unlisted"] {
        let error = block_on(service.start(StartExecution {
            client: client.clone(),
            public_model: PublicModelId::new(model).expect("model"),
            operation: start_operation_for_model(model),
            metadata: access_test_metadata(),
        }))
        .err()
        .expect("unauthorized model");
        assert_eq!(error.kind(), GatewayErrorKind::PolicyDenied);
        assert!(!store.touched.load(Ordering::SeqCst));
    }
    let started = block_on(service.start(StartExecution {
        client: client.clone(),
        public_model: PublicModelId::new("public-alias").expect("model"),
        operation: start_operation_for_model("public-alias"),
        metadata: access_test_metadata(),
    }))
    .expect("authorized alias routes to upstream model");
    drop(started);
    let error = block_on(
        service.start_provider_endpoint(StartProviderExecution {
            client,
            provider: ProviderKind::new("openai").expect("provider"),
            operation: Operation::GenerateImage(ImageRequest::from_raw_json(
                ImageRequestKind::Generation,
                RawJsonPayload::new("openai", Bytes::from_static(br#"{"prompt":"hello"}"#))
                    .expect("image payload"),
            )),
            metadata: access_test_metadata(),
        }),
    )
    .err()
    .expect("model-less endpoint must not bypass the model allowlist");
    assert_eq!(error.kind(), GatewayErrorKind::PolicyDenied);
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

#[test]
fn channel_only_access_groups_list_and_route_only_explicit_sources_and_public_models() {
    use gateway_core::{
        channel::{ChannelBinding, ChannelRevision},
        identity::ChannelId,
        policy::{AccessGroupId, AccessGroupPolicy},
        routing::{
            ModelPresentation,
            source::{SourceId, SourcePolicy, SourcePreference},
        },
    };
    let provider = ProviderKind::new("openai_api").expect("provider");
    let channel = |id: &str| ChannelId::new(id).expect("channel");
    let source_facts = [
        ("chan_a", "model-a", true),
        ("chan_b", "model-b", true),
        ("chan_disabled", "disabled-model", false),
    ];
    for authorized in [
        Some(vec!["chan_a", "chan_disabled"]),
        Some(Vec::new()),
        None,
    ] {
        let group = authorized.as_ref().map(|ids| AccessGroupPolicy {
            id: AccessGroupId::new("access_channels").expect("group"),
            enabled: true,
            limits: RateLimits::unlimited(),
            allowed_models: BTreeSet::from([
                "public-alias".to_owned(),
                "model-b".to_owned(),
                "disabled-model".to_owned(),
            ]),
            pool_group_ids: BTreeSet::new(),
            channel_ids: ids.iter().map(|id| channel(id)).collect(),
        });
        let directory = Arc::new(RuntimeAccountDirectory::default());
        let snapshot = RuntimeSnapshot::new(
            ConfigRevision::new(1).expect("revision"),
            AccountSelectionPolicy::new(
                RotationStrategy::Smart,
                std::num::NonZeroU32::new(1).expect("concurrency"),
                Duration::ZERO,
            ),
            vec![provider.clone()],
            source_facts
                .iter()
                .map(|(id, model, _)| {
                    ProviderModel::new(
                        provider.clone(),
                        UpstreamModelId::new(*model).expect("model"),
                        ModelCapabilities::new(
                            BTreeSet::from([OperationKind::Generate]),
                            Some(16_000),
                        ),
                    )
                    .with_channel(ChannelBinding::new(
                        channel(id),
                        ChannelRevision::new(1).expect("revision"),
                    ))
                    .with_presentation(ModelPresentation::new(Some(model.to_string()), None))
                })
                .collect(),
            vec![
                ClientPolicy::new(
                    ClientApiKeyId::new("key_channels").expect("key"),
                    PlaintextClientApiKey::new("sk_channel_test").expect("secret"),
                    Arc::new(FrozenAccountScope::new(
                        directory,
                        ClientRoutingScope::no_accounts(),
                    )),
                    true,
                    RateLimits::unlimited(),
                )
                .with_access_group(group),
            ],
        )
        .expect("snapshot")
        .with_model_mappings(BTreeMap::from([(
            "public-alias".to_owned(),
            "model-a".to_owned(),
        )]))
        .with_source_policies(
            source_facts
                .iter()
                .map(|(id, _, enabled)| {
                    SourcePolicy::new(
                        SourceId::Channel(channel(id)),
                        *enabled,
                        SourcePreference::default(),
                        RateLimits::unlimited(),
                        None,
                    )
                    .expect("policy")
                })
                .collect(),
        )
        .expect("policies");
        let service = DefaultExecutionService::new(
            RuntimeSnapshotHandle::new(snapshot),
            Arc::new(TrackingExecutionStore::default()),
            ProviderRegistry::default(),
            (
                Arc::new(UnusedAdmissions),
                Arc::new(AllowedSourceAdmissions),
            ),
            Arc::new(UnusedCircuits),
            Arc::new(UnusedContinuation),
            Arc::new(RecordingClientApiKeyUsage::default()),
        );
        let client = service
            .authenticate("sk_channel_test")
            .expect("authenticate");
        let permitted = authorized.as_ref().is_some_and(|ids| !ids.is_empty());
        let expected = if permitted {
            vec!["public-alias"]
        } else {
            Vec::new()
        };
        assert_eq!(
            service
                .public_models(&client)
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            service
                .public_model_profiles(&client)
                .iter()
                .map(|m| m.model().as_str())
                .collect::<Vec<_>>(),
            expected
        );
        for model in ["public-alias", "model-a", "model-b", "disabled-model"] {
            let visible = permitted && model == "public-alias";
            assert_eq!(
                service.contains_public_model(&client, &PublicModelId::new(model).expect("model")),
                visible
            );
            let result = block_on(service.start(StartExecution {
                client: client.clone(),
                public_model: PublicModelId::new(model).expect("model"),
                operation: start_operation_for_model(model),
                metadata: access_test_metadata(),
            }));
            assert_eq!(
                result.is_ok(),
                visible,
                "source and model authorization for {model}"
            );
            drop(result);
        }
    }
}

#[test]
fn each_websocket_generation_rechecks_current_limits_permissions_and_snapshot_availability() {
    use gateway_core::engine::admission::ClientAdmissionRejection;
    use gateway_core::policy::{AccessGroupId, AccessGroupPolicy, AdmissionScopeId};
    let snapshots = RuntimeSnapshotHandle::new(start_snapshot());
    let admissions = Arc::new(RejectingAdmissions::new(
        ClientAdmissionRejection::GlobalConcurrencyLimited,
    ));
    let service = DefaultExecutionService::new(
        snapshots.clone(),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::default(),
        (admissions.clone(), Arc::new(AllowedSourceAdmissions)),
        Arc::new(UnusedCircuits),
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let connection_client = service
        .authenticate("sk_start_test")
        .expect("connection authentication");
    let request = || {
        let mut metadata = access_test_metadata();
        metadata.transport = ClientTransport::WebSocket;
        StartExecution {
            client: connection_client.clone(),
            public_model: PublicModelId::new("gpt-start").expect("model"),
            operation: start_operation(),
            metadata,
        }
    };
    let limits = RateLimits {
        max_concurrency: 1,
        requests_per_minute: 10,
    };
    snapshots.publish(start_snapshot_with_limits(None, limits));
    let error = block_on(service.start(request()))
        .err()
        .expect("new global limit");
    assert_eq!(error.kind(), GatewayErrorKind::NoAvailableProvider);
    let recorded = admissions.requests.lock().expect("admissions");
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0]
            .scopes
            .iter()
            .find(|scope| scope.id == AdmissionScopeId::Global)
            .expect("global")
            .limits,
        limits
    );
    drop(recorded);

    snapshots.publish(start_snapshot_with_access_group(Some(AccessGroupPolicy {
        id: AccessGroupId::new("access_revoked").expect("group"),
        enabled: false,
        limits: RateLimits::unlimited(),
        allowed_models: BTreeSet::from(["gpt-start".to_owned()]),
        channel_ids: std::collections::BTreeSet::new(),
        pool_group_ids: BTreeSet::new(),
    })));
    let error = block_on(service.start(request()))
        .err()
        .expect("group disabled after connection authenticated");
    assert_eq!(error.kind(), GatewayErrorKind::PolicyDenied);
    snapshots.suspend();
    assert_eq!(
        block_on(service.start(request()))
            .err()
            .expect("snapshot suspended")
            .kind(),
        GatewayErrorKind::NoAvailableProvider
    );
    assert_eq!(admissions.requests.lock().expect("admissions").len(), 1);
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
}

fn access_test_metadata() -> ExecutionRequestMetadata {
    ExecutionRequestMetadata {
        protocol: "openai".to_owned(),
        endpoint: "/v1/responses".to_owned(),
        transport: ClientTransport::HttpJson,
        stream: false,
        client_ip: None,
        user_agent: None,
        previous_response_id: None,
    }
}

fn probe_operation() -> Operation {
    let body = json!({
        "model": "gpt-probe",
        "input": [{"type": "message", "role": "user", "content": "ping"}],
    });
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().expect("request object").clone())
            .expect("OpenAI payload"),
    ))
}

fn start_operation() -> Operation {
    start_operation_for_model("gpt-start")
}

fn start_operation_for_model(model: &str) -> Operation {
    let body = json!({
        "model": model,
        "input": [{"type": "message", "role": "user", "content": "ping"}],
    });
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().expect("request object").clone())
            .expect("OpenAI payload"),
    ))
}

#[derive(Default)]
pub(super) struct AllowedSourceAdmissions;
impl gateway_core::engine::source_admission::SourceAdmissionPort for AllowedSourceAdmissions {
    fn acquire(
        &self,
        _: gateway_core::engine::source_admission::SourceAdmissionRequest,
    ) -> futures::future::BoxFuture<
        '_,
        Result<
            Box<dyn gateway_core::engine::provider::ResourceLease>,
            gateway_core::engine::source_admission::SourceAdmissionError,
        >,
    > {
        Box::pin(async {
            Ok(Box::new(()) as Box<dyn gateway_core::engine::provider::ResourceLease>)
        })
    }
}

#[derive(Default)]
struct SourceCircuits {
    blocked: Mutex<BTreeSet<ProviderCircuitScope>>,
    decisions: Mutex<Vec<ProviderCircuitScope>>,
    feedback: Mutex<Vec<(ProviderCircuitScope, bool)>>,
}

struct FixedSourceContinuation(NativeContinuationPin);

impl NativeContinuationPort for FixedSourceContinuation {
    fn resolve<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async { Ok(Some(self.0.clone())) })
    }
    fn record<'a>(
        &'a self,
        _: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

impl ProviderCircuitPort for SourceCircuits {
    fn decision<'a>(
        &'a self,
        scope: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<ProviderCircuitDecision, ProviderCircuitError>> {
        Box::pin(async move {
            self.decisions
                .lock()
                .expect("decisions")
                .push(scope.clone());
            Ok(if self.blocked.lock().expect("blocked").contains(scope) {
                ProviderCircuitDecision::BlockedUntil(SystemTime::now() + Duration::from_secs(30))
            } else {
                ProviderCircuitDecision::Allow
            })
        })
    }
    fn observe_failure<'a>(
        &'a self,
        scope: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async move {
            self.feedback
                .lock()
                .expect("feedback")
                .push((scope.clone(), false));
            self.blocked.lock().expect("blocked").insert(scope.clone());
            Ok(())
        })
    }
    fn observe_success<'a>(
        &'a self,
        scope: &'a ProviderCircuitScope,
    ) -> BoxFuture<'a, Result<(), ProviderCircuitError>> {
        Box::pin(async move {
            self.feedback
                .lock()
                .expect("feedback")
                .push((scope.clone(), true));
            self.blocked.lock().expect("blocked").remove(scope);
            Ok(())
        })
    }
}

fn health_channel(id: &str) -> gateway_core::routing::source::SourceId {
    gateway_core::routing::source::SourceId::Channel(
        gateway_core::identity::ChannelId::new(id).expect("channel"),
    )
}

fn health_snapshot(authorized: &[&str]) -> RuntimeSnapshot {
    use gateway_core::{
        channel::{ChannelBinding, ChannelRevision},
        identity::{ChannelId, QuotaScopeId},
        policy::{AccessGroupId, AccessGroupPolicy},
        routing::source::{QuotaScopePolicy, SourcePolicy, SourcePreference},
    };
    let provider = ProviderKind::new("openai").expect("provider");
    let quota = QuotaScopeId::new("quota_health").expect("quota");
    RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        AccountSelectionPolicy::new(
            RotationStrategy::Smart,
            std::num::NonZeroU32::new(1).expect("concurrency"),
            Duration::from_millis(1),
        ),
        vec![provider.clone()],
        ["chan_a", "chan_b", "chan_hidden"]
            .into_iter()
            .map(|id| {
                ProviderModel::new(
                    provider.clone(),
                    UpstreamModelId::new("gpt-start").expect("model"),
                    ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16_000)),
                )
                .with_channel(ChannelBinding::new(
                    ChannelId::new(id).expect("id"),
                    ChannelRevision::new(1).expect("revision"),
                ))
            })
            .collect(),
        vec![
            ClientPolicy::new(
                ClientApiKeyId::new("key_health").expect("key"),
                PlaintextClientApiKey::new("sk_health").expect("secret"),
                Arc::new(FrozenAccountScope::new(
                    Arc::new(RuntimeAccountDirectory::default()),
                    ClientRoutingScope::no_accounts(),
                )),
                true,
                RateLimits::unlimited(),
            )
            .with_access_group(Some(AccessGroupPolicy {
                id: AccessGroupId::new("access_health").expect("group"),
                enabled: true,
                allowed_models: BTreeSet::from(["gpt-start".to_owned()]),
                pool_group_ids: BTreeSet::new(),
                channel_ids: authorized
                    .iter()
                    .map(|id| ChannelId::new(*id).expect("channel"))
                    .collect(),
                limits: RateLimits::unlimited(),
            })),
        ],
    )
    .expect("snapshot")
    .with_source_policies(
        ["chan_a", "chan_b", "chan_hidden"]
            .into_iter()
            .map(|id| {
                SourcePolicy::new(
                    health_channel(id),
                    true,
                    SourcePreference::default(),
                    RateLimits::unlimited(),
                    Some(quota.clone()),
                )
                .expect("policy")
            })
            .collect(),
    )
    .expect("policies")
    .with_quota_policies(vec![
        QuotaScopePolicy::new(
            quota,
            true,
            RateLimits {
                max_concurrency: 4,
                requests_per_minute: 100,
            },
        )
        .expect("quota"),
    ])
    .expect("quota snapshot")
}

#[derive(Default)]
struct HealthProvider {
    failure: Option<ProviderErrorKind>,
    fail_during_stream: bool,
    calls: Mutex<Vec<gateway_core::routing::source::SourceId>>,
}

#[derive(Default)]
struct LegacySourceGate {
    deny: bool,
    requests: Mutex<Vec<gateway_core::engine::source_admission::SourceAdmissionRequest>>,
}

impl gateway_core::engine::source_admission::SourceAdmissionPort for LegacySourceGate {
    fn acquire(
        &self,
        request: gateway_core::engine::source_admission::SourceAdmissionRequest,
    ) -> BoxFuture<
        '_,
        Result<
            Box<dyn gateway_core::engine::provider::ResourceLease>,
            gateway_core::engine::source_admission::SourceAdmissionError,
        >,
    > {
        Box::pin(async move {
            self.requests.lock().expect("requests").push(request);
            if self.deny {
                return Err(gateway_core::engine::source_admission::SourceAdmissionError::Capacity);
            }
            Ok(Box::new(()) as Box<dyn gateway_core::engine::provider::ResourceLease>)
        })
    }
}

#[test]
fn unbound_keys_and_fixed_account_probes_use_the_pool_admission_and_health_scope() {
    use gateway_core::{
        engine::EngineError,
        routing::{AccountGroupId, source::SourceId},
    };
    let pool = AccountGroupId::new("grp_00000000000000000000000000000001").expect("pool");
    for deny in [true, false] {
        for entry in ["model", "endpoint", "probe"] {
            // The native endpoint path is only needed for the admission rejection case.
            if !deny && entry == "endpoint" {
                continue;
            }
            let snapshot = start_snapshot_with_legacy_pools(
                None,
                RateLimits::unlimited(),
                BTreeSet::from([pool.clone()]),
            );
            let gate = Arc::new(LegacySourceGate {
                deny,
                ..Default::default()
            });
            let provider = Arc::new(HealthProvider::default());
            let circuits = Arc::new(SourceCircuits::default());
            let store = Arc::new(TrackingExecutionStore::default());
            let service = DefaultExecutionService::new(
                RuntimeSnapshotHandle::new(snapshot),
                store.clone(),
                ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).expect("registry"),
                (Arc::new(UnusedAdmissions), gate.clone()),
                circuits.clone(),
                Arc::new(UnusedContinuation),
                Arc::new(RecordingClientApiKeyUsage::default()),
            );
            if entry == "probe" {
                let result = block_on(service.probe(AccountProbeRequest {
                    account_id: ProviderAccountId::new("acct_start").expect("account"),
                    provider_kind: ProviderKind::new("openai").expect("provider"),
                    upstream_model: UpstreamModelId::new("gpt-start").expect("model"),
                    operation: start_operation(),
                }));
                if deny {
                    let error = result.expect_err("probe pool capacity");
                    assert_eq!(error.kind(), GatewayErrorKind::SourceCapacityUnavailable);
                    assert_eq!(error.source(), AccountProbeErrorSource::Gateway);
                    assert_eq!(error.send_state(), Some(UpstreamSendState::NotSent));
                } else {
                    result.expect("probe succeeds");
                }
                assert!(
                    !store.touched.load(Ordering::SeqCst),
                    "probe does not persist a customer request"
                );
                assert_eq!(
                    service
                        .traffic_monitor()
                        .snapshot()
                        .ingress_requests_last_minute,
                    0
                );
            } else {
                let request = health_request(&service, "sk_start_test");
                let mut started = if entry == "endpoint" {
                    block_on(
                        service.start_provider_endpoint(StartProviderExecution {
                            client: request.client,
                            provider: ProviderKind::new("openai").expect("provider"),
                            operation: Operation::GenerateImage(ImageRequest::from_raw_json(
                                ImageRequestKind::Generation,
                                RawJsonPayload::new(
                                    "openai",
                                    Bytes::from_static(br#"{"prompt":"fixture"}"#),
                                )
                                .expect("image"),
                            )),
                            metadata: request.metadata,
                        }),
                    )
                    .expect("start native endpoint")
                } else {
                    block_on(service.start(request)).expect("start model")
                };
                let result = block_on(started.session.collect_uncommitted());
                if deny {
                    assert!(
                        matches!(result, Err(EngineError::Provider(error)) if error.kind() == ProviderErrorKind::SourceCapacityUnavailable)
                    );
                } else {
                    result.expect("legacy key succeeds through pool");
                }
            }
            let acquired = gate.requests.lock().expect("requests");
            assert_eq!(acquired.len(), 1, "{entry}");
            assert_eq!(acquired[0].source, SourceId::AccountPool(pool.clone()));
            if deny {
                assert!(provider.calls.lock().expect("calls").is_empty());
                assert!(circuits.feedback.lock().expect("feedback").is_empty());
            } else {
                assert_eq!(
                    *provider.calls.lock().expect("calls"),
                    [SourceId::AccountPool(pool.clone())]
                );
                assert_eq!(
                    *circuits.feedback.lock().expect("feedback"),
                    [(
                        ProviderCircuitScope::Source(SourceId::AccountPool(pool.clone())),
                        true
                    )]
                );
            }
        }
    }
}

#[async_trait]
impl Provider for HealthProvider {
    fn name(&self) -> &'static str {
        "openai"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(Vec::new())
    }
    async fn execute(
        &self,
        request: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        use gateway_core::{
            event::{GatewayEvent, ProviderEvent, ResponseMeta},
            routing::source::SourceId,
        };
        let candidate = request.candidate();
        let source = candidate.source().expect("source").clone();
        self.calls.lock().expect("calls").push(source.clone());
        let failure = (source == health_channel("chan_a"))
            .then_some(self.failure)
            .flatten();
        if let Some(kind) = failure
            && !self.fail_during_stream
        {
            return Err(ProviderError::new(kind, UpstreamSendState::NotSent));
        }
        let metadata = match source {
            SourceId::Channel(_) => ProviderCallMetadata::for_channel(
                candidate.provider().clone(),
                candidate.upstream_model().cloned(),
                candidate.channel_binding().expect("channel").clone(),
                UpstreamTransport::new("http_sse").expect("transport"),
            ),
            SourceId::AccountPool(_) => ProviderCallMetadata::new(
                candidate.provider().clone(),
                candidate.upstream_model().expect("model").clone(),
                ProviderAccountId::new("acct_start").expect("account"),
                UpstreamTransport::new("http_sse").expect("transport"),
            ),
        };
        let events = match failure {
            Some(kind) => vec![Err(ProviderError::new(kind, UpstreamSendState::Sent))],
            None => vec![
                Ok(ProviderEvent::canonical(GatewayEvent::Started(
                    ResponseMeta::new("resp_health", "gpt-start"),
                ))),
                Ok(ProviderEvent::canonical(GatewayEvent::Completed(
                    ResponseMeta::new("resp_health", "gpt-start"),
                ))),
            ],
        };
        Ok(ProviderStream::new(
            metadata,
            futures::stream::iter(events),
            (),
        ))
    }
}

fn health_service(
    snapshots: RuntimeSnapshotHandle,
    provider: Arc<HealthProvider>,
    circuits: Arc<dyn ProviderCircuitPort>,
) -> DefaultExecutionService {
    DefaultExecutionService::new(
        snapshots,
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::new([provider as Arc<dyn Provider>]).expect("registry"),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        circuits,
        Arc::new(UnusedContinuation),
        Arc::new(RecordingClientApiKeyUsage::default()),
    )
}

fn health_request(service: &DefaultExecutionService, key: &str) -> StartExecution {
    StartExecution {
        client: service.authenticate(key).expect("authenticate"),
        public_model: PublicModelId::new("gpt-start").expect("model"),
        operation: start_operation(),
        metadata: ExecutionRequestMetadata {
            protocol: "openai".to_owned(),
            endpoint: "/v1/responses".to_owned(),
            transport: ClientTransport::HttpJson,
            stream: false,
            client_ip: None,
            user_agent: None,
            previous_response_id: None,
        },
    }
}

#[test]
fn source_health_blocks_only_the_failed_channel_even_when_quota_and_adapter_are_shared() {
    for fail_during_stream in [false, true] {
        let snapshots = RuntimeSnapshotHandle::new(health_snapshot(&["chan_a"]));
        let circuits = Arc::new(SourceCircuits::default());
        let provider = Arc::new(HealthProvider {
            failure: Some(ProviderErrorKind::Transport),
            fail_during_stream,
            ..Default::default()
        });
        let service = health_service(snapshots.clone(), provider.clone(), circuits.clone());
        let mut started =
            block_on(service.start(health_request(&service, "sk_health"))).expect("start A");
        block_on(started.session.collect_uncommitted()).expect_err("A transport failed");
        let a = ProviderCircuitScope::Source(health_channel("chan_a"));
        assert_eq!(
            *circuits.feedback.lock().expect("feedback"),
            [(a.clone(), false)]
        );
        snapshots.publish(health_snapshot(&["chan_a", "chan_b"]));
        circuits.decisions.lock().expect("decisions").clear();
        let mut started =
            block_on(service.start(health_request(&service, "sk_health"))).expect("start B");
        block_on(started.session.collect_uncommitted()).expect("B healthy");
        assert_eq!(
            *provider.calls.lock().expect("calls"),
            [health_channel("chan_a"), health_channel("chan_b")]
        );
        let b = ProviderCircuitScope::Source(health_channel("chan_b"));
        assert_eq!(
            circuits
                .decisions
                .lock()
                .expect("decisions")
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([a.clone(), b.clone()])
        );
        assert_eq!(
            *circuits.feedback.lock().expect("feedback"),
            [(a.clone(), false), (b, true)]
        );
        assert_eq!(
            *circuits.blocked.lock().expect("blocked"),
            BTreeSet::from([a])
        );
    }
}

#[test]
fn all_authorized_sources_blocked_should_not_use_an_unauthorized_channel_or_clear_health() {
    let circuits = Arc::new(SourceCircuits::default());
    for id in ["chan_a", "chan_b"] {
        circuits
            .blocked
            .lock()
            .expect("blocked")
            .insert(ProviderCircuitScope::Source(health_channel(id)));
    }
    let provider = Arc::new(HealthProvider::default());
    let service = health_service(
        RuntimeSnapshotHandle::new(health_snapshot(&["chan_a", "chan_b"])),
        provider.clone(),
        circuits.clone(),
    );
    assert!(block_on(service.start(health_request(&service, "sk_health"))).is_err());
    assert!(provider.calls.lock().expect("calls").is_empty());
    assert!(circuits.feedback.lock().expect("feedback").is_empty());
    assert_eq!(service.traffic_monitor().snapshot().in_flight_requests, 0);
    block_on(circuits.observe_success(&ProviderCircuitScope::Source(health_channel("chan_b"))))
        .expect("recovered B");
    let mut started =
        block_on(service.start(health_request(&service, "sk_health"))).expect("B reopened");
    block_on(started.session.collect_uncommitted()).expect("B completes");
    assert_eq!(
        *provider.calls.lock().expect("calls"),
        [health_channel("chan_b")]
    );
}

#[test]
fn source_capacity_local_coordination_and_client_failures_should_not_change_upstream_health() {
    for kind in [
        ProviderErrorKind::SourceCapacityUnavailable,
        ProviderErrorKind::AccountCapacityUnavailable,
        ProviderErrorKind::ProviderInfrastructureUnavailable,
        ProviderErrorKind::Cancelled,
        ProviderErrorKind::RateLimited,
        ProviderErrorKind::InvalidRequest,
    ] {
        let circuits = Arc::new(SourceCircuits::default());
        let service = health_service(
            RuntimeSnapshotHandle::new(health_snapshot(&["chan_a"])),
            Arc::new(HealthProvider {
                failure: Some(kind),
                ..Default::default()
            }),
            circuits.clone(),
        );
        let mut started =
            block_on(service.start(health_request(&service, "sk_health"))).expect("start");
        block_on(started.session.collect_uncommitted()).expect_err("failed");
        assert!(
            circuits.feedback.lock().expect("feedback").is_empty(),
            "{kind:?}"
        );
    }
}

#[test]
fn source_circuit_read_errors_and_timeouts_should_fail_open_with_one_total_budget() {
    for circuits in [
        Arc::new(FailingDecisionCircuits) as Arc<dyn ProviderCircuitPort>,
        Arc::new(PendingDecisionCircuits),
    ] {
        let provider = Arc::new(HealthProvider::default());
        let service = health_service(
            RuntimeSnapshotHandle::new(health_snapshot(&["chan_a", "chan_b"])),
            provider.clone(),
            circuits,
        );
        let start = Instant::now();
        let mut started =
            block_on(service.start(health_request(&service, "sk_health"))).expect("fail open");
        assert!(start.elapsed() < Duration::from_secs(1));
        block_on(started.session.collect_uncommitted()).expect("healthy source remains available");
        assert_eq!(provider.calls.lock().expect("calls").len(), 1);
    }
}

#[test]
fn pool_health_is_independent_from_other_pools_and_legacy_provider_health() {
    use gateway_core::{
        policy::{AccessGroupId, AccessGroupPolicy},
        routing::{AccountGroupId, source::SourceId},
    };
    let pools: Vec<_> = (1..=2)
        .map(|index| AccountGroupId::new(format!("grp_{index:032x}")).expect("pool"))
        .collect();
    let snapshots =
        RuntimeSnapshotHandle::new(start_snapshot_with_access_group(Some(AccessGroupPolicy {
            id: AccessGroupId::new("access_health_pool").expect("group"),
            enabled: true,
            allowed_models: BTreeSet::from(["gpt-start".to_owned()]),
            pool_group_ids: pools.iter().cloned().collect(),
            channel_ids: BTreeSet::new(),
            limits: RateLimits::unlimited(),
        })));
    let circuits = Arc::new(SourceCircuits::default());
    let legacy = ProviderCircuitScope::Provider(ProviderKind::new("openai").expect("provider"));
    let pool_a = ProviderCircuitScope::Source(SourceId::AccountPool(pools[0].clone()));
    circuits
        .blocked
        .lock()
        .expect("blocked")
        .extend([legacy.clone(), pool_a]);
    let provider = Arc::new(HealthProvider::default());
    let service = health_service(snapshots.clone(), provider.clone(), circuits.clone());
    let mut started = block_on(service.start(health_request(&service, "sk_start_test")))
        .expect("pool B available");
    block_on(started.session.collect_uncommitted()).expect("pool B completes");
    assert_eq!(
        *provider.calls.lock().expect("calls"),
        [SourceId::AccountPool(pools[1].clone())]
    );
    assert!(
        !circuits
            .decisions
            .lock()
            .expect("decisions")
            .contains(&legacy)
    );
    let pin = NativeContinuationPin::new(
        PreviousResponseId::new("resp_previous"),
        PreviousResponseId::new("resp_upstream"),
        ClientApiKeyId::new("key_start_test").expect("key"),
        ProviderKind::new("openai").expect("provider"),
        ProviderAccountId::new("acct_start").expect("account"),
    )
    .with_source(SourceId::AccountPool(pools[0].clone()));
    let pinned_service = DefaultExecutionService::new(
        snapshots.clone(),
        Arc::new(TrackingExecutionStore::default()),
        ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).expect("registry"),
        (
            Arc::new(UnusedAdmissions),
            Arc::new(AllowedSourceAdmissions),
        ),
        circuits.clone(),
        Arc::new(FixedSourceContinuation(pin)),
        Arc::new(RecordingClientApiKeyUsage::default()),
    );
    let mut request = health_request(&pinned_service, "sk_start_test");
    request.metadata.previous_response_id = Some(PreviousResponseId::new("resp_previous"));
    assert!(
        block_on(pinned_service.start(request)).is_err(),
        "native continuation may not escape a blocked pool"
    );
    assert_eq!(provider.calls.lock().expect("calls").len(), 1);
    snapshots.publish(start_snapshot());
    circuits.decisions.lock().expect("decisions").clear();
    assert!(block_on(service.start(health_request(&service, "sk_start_test"))).is_err());
    assert_eq!(*circuits.decisions.lock().expect("decisions"), [legacy]);
    assert_eq!(provider.calls.lock().expect("calls").len(), 1);
}
