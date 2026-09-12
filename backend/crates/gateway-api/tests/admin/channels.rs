use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use gateway_admin::{
    model::{
        MutationContext, Revision,
        channels::{
            ChannelChange, ChannelDiscoveryComparisonQuery, ChannelDiscoveryPage,
            ChannelDiscoveryPair, ChannelDiscoveryQuery, ChannelListQuery, ChannelModelPreview,
            ChannelPage,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ChannelStore},
};
use gateway_core::{
    channel::{ChannelRevision, StoredChannel},
    identity::ChannelId,
};
use serde_json::json;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState, UnusedStore, unavailable};

struct DiscoveryReadStore(Vec<ChannelModelPreview>);

#[async_trait]
impl ChannelStore for DiscoveryReadStore {
    async fn claim_due_model_discovery(
        &self,
    ) -> AdminStoreResult<Option<gateway_admin::model::channels::ChannelDiscoveryClaim>> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "channel",
            "unused schedule",
        ))
    }
    async fn finish_scheduled_model_discovery(
        &self,
        _: &gateway_admin::model::channels::ChannelDiscoveryClaim,
        _: bool,
    ) -> AdminStoreResult<()> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "channel",
            "unused schedule",
        ))
    }
    async fn load_discovery_pair(
        &self,
        query: ChannelDiscoveryComparisonQuery,
    ) -> AdminStoreResult<Option<ChannelDiscoveryPair>> {
        query.validate().expect("validated comparison");
        let find = |generation| {
            self.0
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
        query.validate().expect("validated query");
        let mut items: Vec<_> = self
            .0
            .iter()
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
        Ok(self.0.first().cloned())
    }
    async fn list_channels(&self, _: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        use gateway_admin::model::channels::{
            ChannelDiscoverySchedule, ChannelFields, ChannelRecord,
        };
        let now = chrono::Utc::now();
        Ok(ChannelPage {
            items: vec![ChannelRecord {
                id: ChannelId::new("chan_schedule").expect("id"),
                provider: gateway_core::identity::ProviderKind::new("openai_api")
                    .expect("provider"),
                fields: ChannelFields {
                    name: "Scheduled channel".to_owned(),
                    note: None,
                    enabled: true,
                    preference: gateway_core::policy::SourcePreference::new(1, 1)
                        .expect("preference"),
                    limits: gateway_core::policy::RateLimits {
                        max_concurrency: 0,
                        requests_per_minute: 0,
                    },
                    quota_scope_id: None,
                    discovery_interval_minutes: Some(60),
                },
                connection_revision: ChannelRevision::new(9007199254740993).expect("revision"),
                created_at: now,
                updated_at: now,
                discovery_schedule: ChannelDiscoverySchedule {
                    next_due_at: Some(now + chrono::TimeDelta::minutes(60)),
                    attempted_at: Some(now),
                    completed_at: None,
                    succeeded: None,
                },
            }],
            total: 1,
            config_revision: Revision::new(1).expect("revision"),
        })
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
async fn channel_list_exposes_schedule_without_claiming_work_or_reading_credentials() {
    let fixture =
        AdminTestFixture::with_channel_store(std::sync::Arc::new(DiscoveryReadStore(Vec::new())))
            .await;
    fixture.auth.insert_session("valid-session");
    let response = gateway_api::admin::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(
            Request::builder()
                .uri("/api/admin/channels?page=1&pageSize=20")
                .header("x-request-id", "schedule-read")
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
    let channel = &body["data"]["items"][0];
    assert_eq!(channel["connectionRevision"], "9007199254740993");
    assert_eq!(channel["discoveryIntervalMinutes"], 60);
    assert!(channel["discoverySchedule"]["attemptedAt"].is_string());
    assert!(channel["discoverySchedule"]["nextDueAt"].is_string());
    assert!(channel["discoverySchedule"]["completedAt"].is_null());
    assert!(channel["discoverySchedule"]["succeeded"].is_null());
    assert!(channel.get("config").is_none());
    assert!(channel.get("apiKey").is_none());
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
            DiscoveryReadStore(snapshot.clone().into_iter().collect()),
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

#[tokio::test]
async fn discovery_history_preserves_cursor_precision_and_reads_only_local_records() {
    let older = ChannelModelPreview {
        id: ChannelId::new("chan_saved").expect("id"),
        revision: ChannelRevision::new(9007199254740993).expect("revision"),
        generation: 9007199254740993,
        fetched_at: chrono::Utc::now(),
        added: vec![],
        missing: vec!["old-model".to_owned()],
        unchanged: vec![],
    };
    let mut newer = older.clone();
    newer.generation += 1;
    let fixture =
        AdminTestFixture::with_channel_store(std::sync::Arc::new(DiscoveryReadStore(vec![
            newer, older,
        ])))
        .await;
    fixture.auth.insert_session("valid-session");
    for (cursor, generation, next) in [
        ("", Some("9007199254740994"), Some("9007199254740994")),
        (
            "&beforeGeneration=9007199254740994",
            Some("9007199254740993"),
            None,
        ),
        ("&beforeGeneration=9007199254740993", None, None),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/admin/channels/model-discoveries?id=chan_saved&pageSize=1{cursor}"
                    ))
                    .header("x-request-id", "discovery-history")
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
        assert_eq!(body["data"]["nextBeforeGeneration"], json!(next));
        let items = body["data"]["items"].as_array().expect("items");
        assert_eq!(items.len(), usize::from(generation.is_some()));
        if let Some(generation) = generation {
            assert_eq!(items[0]["generation"], generation);
            assert_eq!(items[0]["connectionRevision"], "9007199254740993");
            assert!(items[0].get("config").is_none());
            assert!(items[0].get("apiKey").is_none());
        }
    }
}

#[tokio::test]
async fn discovery_history_rejects_invalid_queries_and_does_not_hide_store_failure() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (query, expected) in [
        ("", StatusCode::BAD_REQUEST),
        ("id=invalid", StatusCode::BAD_REQUEST),
        ("id=chan_saved&pageSize=0", StatusCode::BAD_REQUEST),
        ("id=chan_saved&pageSize=51", StatusCode::BAD_REQUEST),
        ("id=chan_saved&beforeGeneration=0", StatusCode::BAD_REQUEST),
        (
            "id=chan_saved&beforeGeneration=9223372036854775808",
            StatusCode::BAD_REQUEST,
        ),
        ("id=chan_saved&beforeGeneration=01", StatusCode::BAD_REQUEST),
        (
            "id=chan_saved&beforeGeneration=abc",
            StatusCode::BAD_REQUEST,
        ),
        ("id=chan_saved&unknown=true", StatusCode::BAD_REQUEST),
        ("id=chan_saved", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/admin/channels/model-discoveries?{query}"))
                    .header("x-request-id", "history-validation")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{query}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[async_trait]
impl ChannelStore for UnusedStore {
    async fn claim_due_model_discovery(
        &self,
    ) -> AdminStoreResult<Option<gateway_admin::model::channels::ChannelDiscoveryClaim>> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "channel",
            "unused schedule",
        ))
    }
    async fn finish_scheduled_model_discovery(
        &self,
        _: &gateway_admin::model::channels::ChannelDiscoveryClaim,
        _: bool,
    ) -> AdminStoreResult<()> {
        Err(AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            "channel",
            "unused schedule",
        ))
    }
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

#[tokio::test]
async fn comparison_returns_precise_metadata_and_observed_differences_without_secrets() {
    let base = ChannelModelPreview {
        id: ChannelId::new("chan_compare").expect("id"),
        revision: ChannelRevision::new(9007199254740993).expect("revision"),
        generation: 9007199254740993,
        fetched_at: chrono::Utc::now(),
        added: vec!["a".to_owned()],
        missing: vec!["configured".to_owned()],
        unchanged: vec!["b".to_owned()],
    };
    let mut target = base.clone();
    target.generation += 1;
    target.revision = ChannelRevision::new(9007199254740994).expect("revision");
    target.added = vec!["c".to_owned()];
    let fixture =
        AdminTestFixture::with_channel_store(std::sync::Arc::new(DiscoveryReadStore(vec![
            target, base,
        ])))
        .await;
    fixture.auth.insert_session("valid-session");
    for (id, target_generation, status) in [
        ("chan_compare", "9007199254740994", StatusCode::OK),
        ("chan_other", "9007199254740994", StatusCode::NOT_FOUND),
        ("chan_compare", "9007199254740995", StatusCode::NOT_FOUND),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>().with_state(fixture.state())
            .oneshot(Request::builder()
                .uri(format!("/api/admin/channels/model-discoveries/compare?id={id}&baseGeneration=9007199254740993&targetGeneration={target_generation}"))
                .header("x-request-id", "compare-discoveries")
                .header(header::COOKIE, "cpr_admin_session=valid-session")
                .body(Body::empty()).expect("request"))
            .await.expect("response");
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        if status != StatusCode::OK {
            continue;
        }
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let data = &body["data"];
        assert_eq!(data["id"], "chan_compare");
        assert_eq!(data["base"]["generation"], "9007199254740993");
        assert_eq!(data["target"]["generation"], "9007199254740994");
        assert_eq!(data["base"]["connectionRevision"], "9007199254740993");
        assert!(data["target"]["fetchedAt"].is_string());
        assert_eq!(data["sameConnectionRevision"], false);
        assert_eq!(data["appeared"], json!(["c"]));
        assert_eq!(data["disappeared"], json!(["a"]));
        assert_eq!(data["unchanged"], json!(["b"]));
        for field in ["config", "apiKey", "price", "credentials"] {
            assert!(data.get(field).is_none());
        }
    }
}

#[tokio::test]
async fn comparison_rejects_invalid_generations_and_preserves_unavailable_errors() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for query in [
        "id=chan_compare&baseGeneration=1",
        "id=invalid&baseGeneration=1&targetGeneration=2",
        "id=chan_compare&baseGeneration=0&targetGeneration=2",
        "id=chan_compare&baseGeneration=2&targetGeneration=2",
        "id=chan_compare&baseGeneration=3&targetGeneration=2",
        "id=chan_compare&baseGeneration=01&targetGeneration=2",
        "id=chan_compare&baseGeneration=1&targetGeneration=9223372036854775808",
        "id=chan_compare&baseGeneration=1&targetGeneration=2&config=anything",
        "id=chan_compare&baseGeneration=1&targetGeneration=abc",
        "id=chan_compare&baseGeneration=1&targetGeneration=2",
    ] {
        let expected = if query == "id=chan_compare&baseGeneration=1&targetGeneration=2" {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_REQUEST
        };
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/admin/channels/model-discoveries/compare?{query}"
                    ))
                    .header("x-request-id", "compare-validation")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{query}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
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
        ("GET", "/model-discoveries?id=chan_a"),
        (
            "GET",
            "/model-discoveries/compare?id=chan_a&baseGeneration=1&targetGeneration=2",
        ),
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
    let mut invalid_interval = valid.clone();
    invalid_interval["discoveryIntervalMinutes"] = json!(4);
    let mut too_long = valid.clone();
    too_long["discoveryIntervalMinutes"] = json!(1441);
    for (body, expected) in [
        (missing, StatusCode::BAD_REQUEST),
        (changed, StatusCode::BAD_REQUEST),
        (invalid, StatusCode::BAD_REQUEST),
        (numeric, StatusCode::UNPROCESSABLE_ENTITY),
        (priority, StatusCode::BAD_REQUEST),
        (invalid_interval, StatusCode::BAD_REQUEST),
        (too_long, StatusCode::BAD_REQUEST),
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
