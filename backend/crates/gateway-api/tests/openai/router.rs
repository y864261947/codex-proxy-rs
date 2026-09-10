use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::AUTHORIZATION},
};
use tower::ServiceExt;

use super::api_router_with_origins;
use super::models::ModelsExecution;

const REMOVED_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

#[tokio::test]
async fn realtime_counts_only_business_ingress_and_requires_admin_auth() {
    use gateway_core::engine::traffic::TrafficMonitor;
    use std::sync::Arc;

    let admin = crate::admin::AdminTestFixture::new().await;
    admin.auth.set_api_key("realtime-admin-key");
    let traffic = TrafficMonitor::default();
    let router = gateway_api::initialize(
        gateway_api::ApiConfig {
            asset_directory: std::env::temp_dir(),
            cors_allowed_origins: Vec::new(),
            request_timeout_seconds: None,
            request_id_header: "x-request-id".to_owned(),
        },
        ModelsExecution::new(),
        traffic.clone(),
        admin.services,
        Vec::new(),
        Arc::new(super::EmptyWorkerHealth),
        Arc::new(super::TestLifecycle::default()),
    )
    .expect("API bundle")
    .router();

    for (method, path) in [
        (Method::GET, "/healthz"),
        (Method::GET, "/v1/models"),
        (Method::POST, "/v1/models"),
        (Method::GET, "/v1/responses"),
        (Method::POST, "/v1/unknown"),
    ] {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 0);

    for path in [
        "/v1/responses",
        "/v1/images/generations",
        "/v1/images/edits",
        "/v1/alpha/search",
    ] {
        let response = router
            .clone()
            .oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 4);
    assert_eq!(traffic.snapshot().in_flight_requests, 0);

    let unauthorized = router
        .clone()
        .oneshot(
            Request::get("/api/admin/dashboard/realtime")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let lease = traffic.begin_execution();
    let response = router
        .oneshot(
            Request::get("/api/admin/dashboard/realtime")
                .header("x-api-key", "realtime-admin-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["data"]["scope"], "process");
    assert_eq!(value["data"]["windowSeconds"], 60);
    assert_eq!(value["data"]["ingressRequestsLastMinute"], 4);
    assert_eq!(value["data"]["inFlightRequests"], 1);
    assert_eq!(value["data"]["preparingRequests"], 1);
    drop(lease);
}

#[tokio::test]
async fn removed_responses_review_route_should_not_reach_the_responses_handler() {
    let response = api_router_with_origins(ModelsExecution::new(), Vec::new())
        .await
        .oneshot(
            Request::post("/v1/responses/review")
                .header(AUTHORIZATION, "Bearer sk_models_test")
                .body(Body::empty())
                .expect("build removed review request"),
        )
        .await
        .expect("route removed review request");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn removed_models_catalog_extension_should_use_standard_model_lookup() {
    let response = api_router_with_origins(ModelsExecution::new(), Vec::new())
        .await
        .oneshot(
            Request::get("/v1/models/catalog")
                .header(AUTHORIZATION, "Bearer sk_models_test")
                .body(Body::empty())
                .expect("build removed model catalog request"),
        )
        .await
        .expect("route removed model catalog request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn removed_model_info_route_should_return_not_found() {
    let response = api_router_with_origins(ModelsExecution::new(), Vec::new())
        .await
        .oneshot(
            Request::get("/v1/models/model-a/info")
                .header(AUTHORIZATION, "Bearer sk_models_test")
                .body(Body::empty())
                .expect("build removed model info request"),
        )
        .await
        .expect("route removed model info request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn responses_body_should_accept_payload_above_the_removed_private_limit() {
    let router = api_router_with_origins(ModelsExecution::new(), Vec::new()).await;
    let response = router
        .oneshot(
            Request::post("/v1/responses")
                .body(Body::from(vec![b'a'; REMOVED_BODY_LIMIT_BYTES + 1]))
                .expect("build request above the removed limit"),
        )
        .await
        .expect("route request above the removed limit");

    // The handler sees the body and rejects the missing credentials. A restored body limit
    // would return 413 before authentication runs.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn configured_cors_origin_assembles_router_and_answers_preflight() {
    let router = api_router_with_origins(
        ModelsExecution::new(),
        vec!["https://app.example.com".into()],
    )
    .await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/v1/models")
                .header("origin", "https://app.example.com")
                .header("access-control-request-method", "GET")
                .header("access-control-request-headers", "authorization")
                .body(Body::empty())
                .expect("build preflight request"),
        )
        .await
        .expect("route preflight request");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://app.example.com")
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-credentials")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    let allow_headers = response
        .headers()
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .expect("allow-headers present");
    assert!(allow_headers.contains("authorization"));
    assert!(allow_headers.contains("x-api-key"));
    assert!(allow_headers.contains("x-request-id"));
}
