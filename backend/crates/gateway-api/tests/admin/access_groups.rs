use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState};

#[tokio::test]
async fn access_group_routes_require_admin_auth_and_prevent_cached_responses() {
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
                    .uri(format!("/api/admin/access-groups{path}"))
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
async fn access_group_assignment_requires_an_explicit_binding_and_valid_access_group_id() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (body, expected) in [
        (
            json!({"keyId": "key_one"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"keyId": "key_one", "accessGroupId": ""}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"keyId": "key_one", "accessGroupId": null, "other": true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"keyId": "key_one", "accessGroupId": null}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            json!({"keyId": "key_one", "accessGroupId": "access_one"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/access-groups/assign-key")
                    .header("x-request-id", "req_access_group_assignment")
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

#[tokio::test]
async fn access_group_creation_requires_explicit_permissions_and_valid_pool_ids() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let valid = json!({"name":"Team", "enabled":true, "maxConcurrency":4, "requestsPerMinute":60, "allowedModels":["model"], "poolGroupIds":[], "channelIds":[]});
    for (field, value, expected) in [
        ("poolGroupIds", json!(["invalid"]), StatusCode::BAD_REQUEST),
        ("channelIds", json!(["invalid"]), StatusCode::BAD_REQUEST),
        (
            "channelIds",
            serde_json::Value::Null,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("allowedModels", json!(["*"]), StatusCode::BAD_REQUEST),
        ("allowedModels", json!([]), StatusCode::SERVICE_UNAVAILABLE),
        (
            "poolGroupIds",
            serde_json::Value::Null,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut body = valid.clone();
        body[field] = value;
        let response = gateway_api::admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/access-groups/create")
                    .header("x-request-id", "req_access_create")
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
