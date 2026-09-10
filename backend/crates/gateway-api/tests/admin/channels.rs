use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use gateway_admin::{
    model::{
        MutationContext, Revision,
        channels::{ChannelChange, ChannelListQuery, ChannelPage},
    },
    ports::store::{AdminStoreResult, ChannelStore},
};
use gateway_core::{channel::StoredChannel, identity::ChannelId};
use serde_json::json;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState, UnusedStore, unavailable};

#[async_trait]
impl ChannelStore for UnusedStore {
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
        ("POST", "/create"),
        ("POST", "/update"),
        ("POST", "/delete"),
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
