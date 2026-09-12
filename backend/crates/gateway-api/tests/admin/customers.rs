use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState};

#[tokio::test]
async fn customer_routes_require_admin_auth_and_prevent_cached_responses() {
    let fixture = AdminTestFixture::new().await;
    for (method, path) in [
        ("GET", ""),
        ("POST", "/create"),
        ("POST", "/update"),
        ("POST", "/delete"),
        ("POST", "/assign-key"),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/admin/customers{path}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache header"),
            "no-store"
        );
    }
}

#[tokio::test]
async fn customer_assignment_requires_an_explicit_binding_and_valid_customer_id() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (body, expected) in [
        (
            json!({"keyId": "key_one"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"keyId": "key_one", "customerId": ""}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"keyId": "key_one", "customerId": null, "other": true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"keyId": "key_one", "customerId": null}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            json!({"keyId": "key_one", "customerId": "cust_one"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/customers/assign-key")
                    .header("x-request-id", "req_customer_assignment")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{body}");
    }
}
