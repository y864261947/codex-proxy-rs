use std::{
    collections::BTreeMap,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use futures::{StreamExt as _, future::BoxFuture};
use gateway_core::{
    channel::{
        ChannelBinding, ChannelRevision, ChannelStorePort, ProviderChannelConfig, StoredChannel,
    },
    engine::{
        AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
        provider::{Provider as _, ProviderRequest},
    },
    error::ProviderErrorKind,
    event::GatewayEvent,
    identity::{ChannelId, ProviderKind},
    lifecycle::CancellationToken,
    operation::{GenerateRequest, Operation, OperationKind, ProtocolPayload},
    policy::{ClientApiKeyId, RateLimits},
    provider_ports::ProviderStoreError,
    routing::{
        ClientRoutingScope, ConfigRevision, FrozenAccountScope, ModelCapabilities, ProviderModel,
        PublicModelId, RoutingContext, RuntimeAccountDirectory, RuntimeSnapshot, UpstreamModelId,
        source::{AllowedSources, SourceId, SourcePolicy, SourcePreference},
    },
    upstream::UpstreamSendState,
};
use provider_openai::api::provider::ApiChannelProvider;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use crate::support::account_policy;

const SECRET: &str = "sk_channel_test_secret_only";
const SSE: &str = concat!(
    "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_api\",\"model\":\"gpt-5.4\"}}\n\n",
    "event: response.future\ndata: {\"type\":\"response.future\",\"extra\":42}\n\n",
    "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"hello\"}\n\n",
    "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_api\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":4},\"output_tokens\":2,\"output_tokens_details\":{\"reasoning_tokens\":0},\"total_tokens\":12}}}\n\n",
);

#[derive(Default)]
struct Store(Mutex<Vec<StoredChannel>>);
impl ChannelStorePort for Store {
    fn load_channel<'a>(
        &'a self,
        id: &'a ChannelId,
        revision: ChannelRevision,
    ) -> BoxFuture<'a, Result<Option<StoredChannel>, ProviderStoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .expect("store")
                .iter()
                .find(|channel| channel.id == *id && channel.revision == revision)
                .cloned())
        })
    }
    fn list_enabled_channels<'a>(
        &'a self,
        provider: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<Vec<StoredChannel>, ProviderStoreError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .expect("store")
                .iter()
                .filter(|channel| channel.provider == *provider)
                .cloned()
                .collect())
        })
    }
}

fn channel(id: &str, revision: u64, base_url: &str) -> StoredChannel {
    StoredChannel {
        id: ChannelId::new(id).expect("id"), provider: ProviderKind::new("openai_api").expect("provider"),
        revision: ChannelRevision::new(revision).expect("revision"),
        config: ProviderChannelConfig::new(json!({"baseUrl":format!("{base_url}/prefix/v1"),"apiKey":SECRET,"models":["gpt-5.4"],"organization":"org_test","project":"proj_test"}).as_object().expect("object").clone()).expect("config"),
    }
}

fn context(cancellation: CancellationToken) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_api_channel").expect("request"),
            ClientApiKeyId::new("key_api_channel").expect("key"),
        ),
        NonZeroU32::new(1).expect("attempt"),
        SystemTime::now() + Duration::from_secs(20),
        account_policy(),
        AccountAttemptContext::default(),
        None,
        cancellation,
    )
}

fn request(channel: &StoredChannel, extra: Value) -> ProviderRequest {
    let mut body = json!({"model":"public-alias","input":"hello","temperature":0.4,"max_output_tokens":90,"stream":false}).as_object().expect("body").clone();
    body.extend(extra.as_object().expect("extras").clone());
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(ProtocolPayload::json_object("openai", body).expect("payload").with_context(json!({"opaque_request_headers":{"Authorization":"Bearer downstream_secret","Cookie":"downstream_cookie"}}).as_object().expect("context").clone())));
    let binding = ChannelBinding::new(channel.id.clone(), channel.revision);
    let source = SourceId::Channel(channel.id.clone());
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        account_policy(),
        vec![channel.provider.clone()],
        vec![
            ProviderModel::new(
                channel.provider.clone(),
                UpstreamModelId::new("gpt-5.4").expect("model"),
                ModelCapabilities::new([OperationKind::Generate].into(), None)
                    .with_upstream_feature_validation(),
            )
            .with_channel(binding),
        ],
        vec![],
    )
    .expect("snapshot")
    .with_source_policies(vec![
        SourcePolicy::new(
            source.clone(),
            true,
            SourcePreference::default(),
            RateLimits::unlimited(),
            None,
        )
        .expect("source"),
    ])
    .expect("policies")
    .with_model_mappings(BTreeMap::from([(
        "public-alias".to_owned(),
        "gpt-5.4".to_owned(),
    )]));
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::default()),
        ClientRoutingScope::no_accounts(),
    ));
    let plan = snapshot
        .plan_channels(
            &PublicModelId::new("public-alias").expect("public"),
            &operation,
            scope,
            &RoutingContext::default(),
            &AllowedSources::new([source]),
        )
        .expect("plan");
    ProviderRequest::new(operation, plan.candidates()[0].clone())
}

#[tokio::test]
async fn catalog_is_channel_scoped_versioned_and_never_fetches_the_upstream() {
    let server = MockServer::start().await;
    let store = Arc::new(Store::default());
    *store.0.lock().expect("store") = vec![
        channel("chan_a", 1, &server.uri()),
        channel("chan_b", 3, &server.uri()),
    ];
    let provider = ApiChannelProvider::new(store.clone()).expect("provider");
    let models = provider.query_model_capabilities().await.expect("catalog");
    assert_eq!(models.len(), 2);
    assert_ne!(models[0].channel_binding(), models[1].channel_binding());
    let generation = provider.catalog_generation();
    provider
        .query_model_capabilities()
        .await
        .expect("unchanged");
    assert_eq!(provider.catalog_generation(), generation);
    store.0.lock().expect("store")[0].revision = ChannelRevision::new(2).expect("revision");
    provider.query_model_capabilities().await.expect("changed");
    assert_ne!(provider.catalog_generation(), generation);
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn cold_channel_call_preserves_protocol_parameters_usage_and_wire_without_oauth_headers_or_prices()
 {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", "upstream_api_req")
                .set_body_raw(SSE, "text/event-stream; charset=utf-8"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let channel = channel("chan_a", 1, &server.uri());
    let store = Arc::new(Store(Mutex::new(vec![channel.clone()])));
    let provider = ApiChannelProvider::new(store).expect("provider");
    let request = request(&channel, json!({}));
    let stream = provider
        .execute(request.clone(), context(CancellationToken::new()))
        .await
        .expect("cold stream");
    assert!(stream.metadata().confirms(request.candidate()));
    assert!(stream.metadata().provider_account_id().is_none());
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
    let events = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events");
    let facts = events
        .iter()
        .flat_map(|event| event.canonical_facts())
        .collect::<Vec<_>>();
    assert!(
        facts
            .iter()
            .any(|event| matches!(event, GatewayEvent::Usage(_)))
    );
    assert!(
        facts
            .iter()
            .any(|event| matches!(event, GatewayEvent::Completed(_)))
    );
    assert!(
        !facts
            .iter()
            .any(|event| matches!(event, GatewayEvent::CalculatedCost(_)))
    );
    let wire: Vec<_> = events
        .iter()
        .filter_map(|event| event.wire_event())
        .filter_map(|wire| wire.raw_sse_frame())
        .flat_map(|frame| frame.iter().copied())
        .collect();
    assert_eq!(wire, SSE.as_bytes());
    assert!(
        events
            .iter()
            .filter_map(|event| event.response_observation())
            .any(|observation| observation.timings().first_text_ms.is_some())
    );
    let received = server.received_requests().await.expect("requests");
    let sent = &received[0];
    assert_eq!(sent.headers["authorization"], format!("Bearer {SECRET}"));
    assert_eq!(sent.headers["openai-organization"], "org_test");
    assert_eq!(sent.headers["openai-project"], "proj_test");
    for header in [
        "cookie",
        "chatgpt-account-id",
        "origin",
        "content-encoding",
        "x-codex-turn-state",
    ] {
        assert!(!sent.headers.contains_key(header));
    }
    let body: Value = serde_json::from_slice(&sent.body).expect("sent body");
    assert_eq!(body["model"], "gpt-5.4");
    assert_eq!(body["temperature"], 0.4);
    assert_eq!(body["max_output_tokens"], 90);
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn rotated_disabled_and_unsupported_continuations_never_send() {
    let server = MockServer::start().await;
    let original = channel("chan_a", 1, &server.uri());
    let store = Arc::new(Store(Mutex::new(vec![channel("chan_a", 2, &server.uri())])));
    let provider = ApiChannelProvider::new(store.clone()).expect("provider");
    let error = provider
        .execute(
            request(&original, json!({})),
            context(CancellationToken::new()),
        )
        .await
        .err()
        .expect("stale rejected");
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    store.0.lock().expect("store").clear();
    assert!(
        provider
            .execute(
                request(&original, json!({})),
                context(CancellationToken::new())
            )
            .await
            .is_err()
    );
    store.0.lock().expect("store").push(original.clone());
    for extra in [
        json!({"previous_response_id":"resp_elsewhere"}),
        json!({"conversation":"conv_elsewhere"}),
        json!({"background":true}),
    ] {
        assert!(
            provider
                .execute(request(&original, extra), context(CancellationToken::new()))
                .await
                .is_err()
        );
    }
    let cancellation = CancellationToken::new();
    let mut stream = provider
        .execute(request(&original, json!({})), context(cancellation.clone()))
        .await
        .expect("prepared");
    cancellation.cancel();
    let error = stream
        .next()
        .await
        .expect("cancelled event")
        .expect_err("cancelled");
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn errors_and_redirects_are_single_attempt_and_never_follow_a_new_origin() {
    let target = MockServer::start().await;
    for status in [401, 429, 500, 307] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("location", format!("{}/responses", target.uri()))
                    .insert_header("retry-after", "9")
                    .set_body_string(SECRET),
            )
            .expect(1)
            .mount(&server)
            .await;
        let channel = channel("chan_a", 1, &server.uri());
        let provider = ApiChannelProvider::new(Arc::new(Store(Mutex::new(vec![channel.clone()]))))
            .expect("provider");
        let events = provider
            .execute(
                request(&channel, json!({})),
                context(CancellationToken::new()),
            )
            .await
            .expect("cold")
            .collect::<Vec<_>>()
            .await;
        let error = events.into_iter().find_map(Result::err).expect("failed");
        assert_eq!(error.upstream_status(), Some(status));
        assert_eq!(error.send_state(), UpstreamSendState::Sent);
        assert!(!format!("{error:?}").contains(SECRET));
        if status == 429 {
            assert_eq!(error.kind(), ProviderErrorKind::RateLimited);
            assert_eq!(error.retry_after(), Some(Duration::from_secs(9)));
        }
    }
    assert!(
        target
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn incomplete_sse_and_cancelled_inflight_calls_cannot_be_reclassified_as_unsent() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_raw("event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_partial\"}}\n\n", "text/event-stream")).expect(1).mount(&server).await;
    let channel = channel("chan_a", 1, &server.uri());
    let provider = ApiChannelProvider::new(Arc::new(Store(Mutex::new(vec![channel.clone()]))))
        .expect("provider");
    let events = provider
        .execute(
            request(&channel, json!({})),
            context(CancellationToken::new()),
        )
        .await
        .expect("cold")
        .collect::<Vec<_>>()
        .await;
    assert!(
        events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .any(|event| event.wire_event().is_some())
    );
    let error = events.into_iter().find_map(Result::err).expect("truncated");
    assert_eq!(error.kind(), ProviderErrorKind::Protocol);
    assert_eq!(error.send_state(), UpstreamSendState::Sent);
    use tokio::io::AsyncReadExt as _;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let channel = self::channel("chan_cancel", 1, &format!("http://{address}"));
    let provider = ApiChannelProvider::new(Arc::new(Store(Mutex::new(vec![channel.clone()]))))
        .expect("provider");
    let cancellation = CancellationToken::new();
    let stream = provider
        .execute(request(&channel, json!({})), context(cancellation.clone()))
        .await
        .expect("cold");
    let (release, released) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accepted");
        let mut buffer = [0_u8; 4096];
        let received = socket.read(&mut buffer).await.expect("request bytes");
        assert!(received > 0);
        cancellation.cancel();
        let _ = released.await;
        drop(socket);
    });
    let events = tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
        .await
        .expect("cancellation completes while the upstream socket remains open");
    release.send(()).expect("release upstream");
    server_task.await.expect("upstream task");
    let error = events.into_iter().find_map(Result::err).expect("cancelled");
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert_eq!(error.send_state(), UpstreamSendState::Ambiguous);
}
