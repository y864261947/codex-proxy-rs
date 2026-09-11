use std::{collections::BTreeSet, num::NonZeroU32, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use gateway_core::{
    account::{AccountSelectionPolicy, RotationStrategy},
    channel::{ChannelBinding, ChannelRevision},
    identity::{ChannelId, ProviderKind, SourceId},
    operation::OperationKind,
    policy::{RateLimits, SourcePreference},
    routing::{
        ConfigRevision, ModelCapabilities, ProviderModel, RuntimeSnapshot, UpstreamModelId,
        source::SourcePolicy,
    },
};
use serde_json::Value;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState};

#[tokio::test]
async fn catalog_requires_auth_validates_filters_and_does_not_mask_unavailable_snapshots() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (query, authenticated, expected) in [
        ("", false, StatusCode::UNAUTHORIZED),
        ("", true, StatusCode::SERVICE_UNAVAILABLE),
        ("?page=0", true, StatusCode::BAD_REQUEST),
        ("?pageSize=201", true, StatusCode::BAD_REQUEST),
        ("?sourceKind=invalid", true, StatusCode::BAD_REQUEST),
        ("?unknown=1", true, StatusCode::BAD_REQUEST),
    ] {
        let mut request = Request::builder().uri(format!("/api/admin/model-catalog{query}"));
        if authenticated {
            request = request.header(header::COOKIE, "cpr_admin_session=valid-session");
        }
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{query}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn catalog_keeps_same_name_sources_unknown_capabilities_and_large_versions_explicit() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let provider = ProviderKind::new("openai_api").expect("provider");
    let channels = ["chan_one", "chan_two"].map(|id| ChannelId::new(id).expect("channel"));
    fixture.model_catalog.publish(
        RuntimeSnapshot::new(
            ConfigRevision::new(9007199254740993).expect("revision"),
            AccountSelectionPolicy::new(
                RotationStrategy::Smart,
                NonZeroU32::new(1).expect("concurrency"),
                Duration::ZERO,
            ),
            vec![provider.clone()],
            channels
                .iter()
                .map(|channel| {
                    ProviderModel::new(
                        provider.clone(),
                        UpstreamModelId::new("same-model").expect("model"),
                        ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), None),
                    )
                    .with_channel(ChannelBinding::new(
                        channel.clone(),
                        ChannelRevision::new(9007199254740993).expect("version"),
                    ))
                })
                .collect(),
            Vec::new(),
        )
        .expect("snapshot")
        .with_source_policies(
            channels
                .into_iter()
                .map(|channel| {
                    SourcePolicy::new(
                        SourceId::Channel(channel),
                        true,
                        SourcePreference::default(),
                        RateLimits::unlimited(),
                        None,
                    )
                    .expect("policy")
                })
                .collect(),
        )
        .expect("policies"),
    );
    let response = gateway_api::admin::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(
            Request::builder()
                .uri("/api/admin/model-catalog")
                .header(header::COOKIE, "cpr_admin_session=valid-session")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.expect("body"))
            .expect("json");
    let data = &body["data"];
    assert_eq!(data["configRevision"], "9007199254740993");
    assert_eq!(data["total"], 2);
    let items = data["items"].as_array().expect("items");
    assert_ne!(items[0]["identityKey"], items[1]["identityKey"]);
    for item in items {
        assert_eq!(item["source"]["connectionRevision"], "9007199254740993");
        assert_eq!(item["features"]["vision"], "unknown");
        assert!(item["contextWindowTokens"].is_null());
        assert_eq!(item["source"]["kind"], "channel");
        assert!(item.get("price").is_none());
        assert!(item.get("config").is_none());
        assert!(item.get("apiKey").is_none());
    }
}
