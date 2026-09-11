use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use gateway_admin::{
    model::{
        MutationContext, Revision,
        channels::{ChannelChange, ChannelListQuery, ChannelModelPreview, ChannelPage},
    },
    ports::store::{AdminStoreResult, ChannelStore},
};
use gateway_core::{
    channel::{ChannelRevision, StoredChannel},
    identity::ChannelId,
};
use serde_json::json;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState, UnusedStore, unavailable};

struct DiscoveryReadStore(Option<ChannelModelPreview>);

#[async_trait]
impl ChannelStore for DiscoveryReadStore {
    async fn reserve_model_discovery(
        &self,
        _: &ChannelId,
        _: ChannelRevision,
    ) -> AdminStoreResult<u64> {
        panic!("GET must not start discovery")
    }
    async fn save_model_discovery(&self, _: &ChannelModelPreview) -> AdminStoreResult<()> {
        panic!("GET must not write")
    }
    async fn load_model_discovery(
        &self,
        _: &ChannelId,
    ) -> AdminStoreResult<Option<ChannelModelPreview>> {
        Ok(self.0.clone())
    }
    async fn list_channels(&self, _: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        Err(unavailable("unused"))
    }
    async fn load_channel_for_edit(
        &self,
        _: &ChannelId,
    ) -> AdminStoreResult<Option<StoredChannel>> {
        panic!("GET must not load credentials")
    }
    async fn change_channel(
        &self,
        _: ChannelChange,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        panic!("GET must not change config")
    }
}

#[tokio::test]
async fn saved_discovery_get_is_local_nullable_and_preserves_large_versions() {
    let preview = ChannelModelPreview {
        id: ChannelId::new("chan_saved").expect("id"),
        revision: ChannelRevision::new(9007199254740993).expect("revision"),
        generation: 9007199254740995,
        fetched_at: chrono::Utc::now(),
        added: vec!["new".to_owned()],
        missing: vec!["missing".to_owned()],
        unchanged: vec![],
    };
    for snapshot in [None, Some(preview)] {
        let fixture = AdminTestFixture::with_channel_store(std::sync::Arc::new(
            DiscoveryReadStore(snapshot.clone()),
        ))
        .await;
        fixture.auth.insert_session("valid-session");
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri("/api/admin/channels/model-discovery?id=chan_saved")
                    .header("x-request-id", "discovery-read")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        if snapshot.is_some() {
            assert_eq!(body["data"]["connectionRevision"], "9007199254740993");
            assert_eq!(body["data"]["generation"], "9007199254740995");
            assert_eq!(body["data"]["missing"], json!(["missing"]));
            assert!(body["data"].get("config").is_none());
            assert!(body["data"].get("apiKey").is_none());
        } else {
            assert!(body["data"].is_null());
        }
    }
}

#[tokio::test]
async fn discovery_requires_saved_identity_and_string_revision() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (body, expected) in [
        (
            json!({"id":"chan_test", "expectedRevision":"0"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"id":"invalid", "expectedRevision":"1"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"id":"chan_test", "expectedRevision":1}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"id":"chan_test", "expectedRevision":"1", "baseUrl":"https://other.invalid"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"id":"chan_test", "expectedRevision":"1"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/channels/discover-models")
                    .header("x-request-id", "discovery-query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[async_trait]
impl ChannelStore for UnusedStore {
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

#[tokio::test]
async fn channel_endpoints_require_admin_auth_and_never_cache_credentials() {
    let fixture = AdminTestFixture::new().await;
    for (method, path) in [
        ("GET", ""),
        ("GET", "/providers"),
        ("GET", "/connection?id=chan_a"),
        ("GET", "/model-discovery?id=chan_a"),
        ("POST", "/create"),
        ("POST", "/update"),
        ("POST", "/delete"),
        ("POST", "/discover-models"),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/admin/channels{path}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn channel_update_requires_explicit_revision_and_keeps_provider_identity_immutable() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let valid = json!({"id": "chan_test", "expectedRevision": "1", "name": "A", "enabled": true, "priority": 1, "weight": 1, "maxConcurrency": 0, "requestsPerMinute": 0});
    let mut missing = valid.clone();
    missing
        .as_object_mut()
        .expect("object")
        .remove("expectedRevision");
    let mut changed = valid.clone();
    changed["provider"] = json!("openai_api");
    let mut invalid = valid.clone();
    invalid["expectedRevision"] = json!("0");
    let mut numeric = valid.clone();
    numeric["expectedRevision"] = json!(1);
    let mut priority = valid.clone();
    priority["priority"] = json!(0);
    for (body, expected) in [
        (missing, StatusCode::BAD_REQUEST),
        (changed, StatusCode::BAD_REQUEST),
        (invalid, StatusCode::BAD_REQUEST),
        (numeric, StatusCode::UNPROCESSABLE_ENTITY),
        (priority, StatusCode::BAD_REQUEST),
        (valid, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/channels/update")
                    .header("x-request-id", "req_channel_update")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{body}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
