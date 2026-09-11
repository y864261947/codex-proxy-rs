use gateway_admin::ports::{channels::ChannelProviderAdmin, provider::ProviderAdminErrorKind};
use gateway_core::channel::ProviderChannelConfig;
use provider_openai::api::admin::ApiChannelAdmin;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn config(base: &str) -> ProviderChannelConfig {
    ProviderChannelConfig::new(
        json!({
            "baseUrl": format!("{base}/prefix/v1"),
            "apiKey": "sk_fixture_discovery_only",
            "models": ["existing", "missing"],
            "organization": "org_fixture", "project": "proj_fixture"
        })
        .as_object()
        .expect("object")
        .clone(),
    )
    .expect("config")
}

#[tokio::test]
async fn discovery_uses_only_saved_channel_headers_and_keeps_config_separate() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list", "data": [{"id": "new-model", "price": 0}, {"id":"existing"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let stored = config(&server.uri());
    let result = ApiChannelAdmin::default()
        .discover_models(&stored)
        .await
        .expect("discovery");
    assert_eq!(
        result
            .configured
            .iter()
            .map(|model| model.as_str())
            .collect::<Vec<_>>(),
        ["existing", "missing"]
    );
    assert_eq!(
        result
            .discovered
            .iter()
            .map(|model| model.as_str())
            .collect::<Vec<_>>(),
        ["existing", "new-model"]
    );
    assert_eq!(
        stored.expose_to_provider()["models"],
        json!(["existing", "missing"])
    );
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;
    assert_eq!(headers["authorization"], "Bearer sk_fixture_discovery_only");
    assert_eq!(headers["openai-organization"], "org_fixture");
    assert_eq!(headers["openai-project"], "proj_fixture");
    assert!(!headers.contains_key("cookie"));
    assert!(!headers.contains_key("chatgpt-account-id"));
    assert!(requests[0].body.is_empty());
}

#[tokio::test]
async fn discovery_rejects_partial_invalid_duplicate_and_oversized_catalogs() {
    let cases: Vec<Value> = vec![
        json!({"object":"list", "data":[{"id":"first"}], "has_more":true}),
        json!({"object":"list", "data":[], "next":"https://other.invalid/secret"}),
        json!({"object":"list", "data":[], "next_cursor":"cursor"}),
        json!({"object":"list", "data":[], "next_page":2}),
        json!({"object":"list", "data":[], "pagination":{"hasMore":true}}),
        json!({"object":"list", "data":[], "links":{"next":"/models"}}),
        json!({"object":"list", "data":[{"id":"repeat"},{"id":"repeat"}]}),
        json!({"object":"list", "data":[{"id":" padded"}]}),
        json!({"object":"list", "data":[{"id":""}]}),
        json!({"object":"list", "data":[{}]}),
        json!({"object":"list"}),
        json!({"data":[]}),
        json!({"object":"list", "data":(0..1001).map(|index| json!({"id":format!("model-{index}")})).collect::<Vec<_>>()}),
        json!({"object":"list", "data":[], "padding":"x".repeat(2 * 1024 * 1024)}),
    ];
    for body in cases {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/prefix/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = ApiChannelAdmin::default()
            .discover_models(&config(&server.uri()))
            .await
            .expect_err("reject malformed catalog");
        assert_eq!(error.kind(), ProviderAdminErrorKind::BadGateway);
        assert!(error.message().is_none());
    }
}

#[tokio::test]
async fn discovery_never_follows_redirects_and_does_not_expose_upstream_errors() {
    let destination = MockServer::start().await;
    for status in [302, 401, 403, 429, 500] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/prefix/v1/models"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("Location", format!("{}/leak", destination.uri()))
                    .set_body_string("sk_fixture_discovery_only"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = ApiChannelAdmin::default()
            .discover_models(&config(&server.uri()))
            .await
            .expect_err("upstream failure");
        assert!(error.message().is_none());
        assert!(!format!("{error:?}").contains("sk_fixture"));
    }
    assert!(
        destination
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
}

#[tokio::test]
async fn successful_empty_discovery_does_not_remove_configured_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"list", "data":[]})))
        .mount(&server)
        .await;
    let result = ApiChannelAdmin::default()
        .discover_models(&config(&server.uri()))
        .await
        .expect("empty success");
    assert!(result.discovered.is_empty());
    assert_eq!(result.configured.len(), 2);
}

#[tokio::test]
async fn discovery_rejects_header_only_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Link", "</prefix/v1/models?page=2>; rel=\"next\"")
                .set_body_json(json!({"object":"list", "data":[{"id":"first-page"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = ApiChannelAdmin::default()
        .discover_models(&config(&server.uri()))
        .await
        .expect_err("incomplete page");
    assert_eq!(error.kind(), ProviderAdminErrorKind::BadGateway);
}
