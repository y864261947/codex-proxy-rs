use super::{AdminTestFixture, AdminTestState};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt as _;

#[tokio::test]
async fn shared_quota_routes_require_admin_auth_and_no_store() {
    let fixture = AdminTestFixture::new().await;
    for (method, suffix) in [
        ("GET", ""),
        ("POST", "/create"),
        ("POST", "/update"),
        ("POST", "/delete"),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/admin/quota-scopes{suffix}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn shared_quota_writes_reject_invalid_identity_limits_and_unknown_fields() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let valid =
        json!({"name":"Project quota", "enabled":true, "maxConcurrency":2, "requestsPerMinute":60});
    let mut excessive = valid.clone();
    excessive["maxConcurrency"] = json!(9_007_199_254_740_992_u64);
    let mut blank = valid.clone();
    blank["name"] = json!("");
    let mut unknown = valid.clone();
    unknown["apiKey"] = json!("must-not-be-accepted");
    let mut custom_id = valid.clone();
    custom_id["id"] = json!("quota_custom");
    let mut invalid_id = valid.clone();
    invalid_id["id"] = json!("cust_invalid");
    for (suffix, body, expected) in [
        ("create", valid, StatusCode::SERVICE_UNAVAILABLE),
        ("create", excessive, StatusCode::BAD_REQUEST),
        ("create", blank, StatusCode::BAD_REQUEST),
        ("create", unknown, StatusCode::UNPROCESSABLE_ENTITY),
        ("create", custom_id, StatusCode::BAD_REQUEST),
        ("update", invalid_id, StatusCode::BAD_REQUEST),
        ("delete", json!({"id":""}), StatusCode::BAD_REQUEST),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/admin/quota-scopes/{suffix}"))
                    .header("x-request-id", "req_quota_test")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{suffix}: {body}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
