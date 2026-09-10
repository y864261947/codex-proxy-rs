use std::sync::Arc;

use gateway_admin::{
    model::provider_credentials::ProviderDocument,
    ports::{
        channels::{ChannelAdminRegistry, ChannelProviderAdmin},
        provider::ProviderAdminErrorKind,
    },
};
use gateway_core::{account::OpaqueProviderData, identity::ProviderKind};
use provider_openai::api::admin::ApiChannelAdmin;
use serde_json::{Value, json};

fn document(value: Value) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(
        value.as_object().expect("object").clone(),
    ))
}

#[test]
fn api_channel_admin_dispatch_is_separate_from_oauth_and_exposes_only_public_fields() {
    let provider: Arc<dyn ChannelProviderAdmin> = Arc::new(ApiChannelAdmin::default());
    let registry = ChannelAdminRegistry::new([Arc::clone(&provider)]).expect("registry");
    assert_eq!(
        registry
            .kinds()
            .map(ProviderKind::as_str)
            .collect::<Vec<_>>(),
        ["openai_api"]
    );
    assert!(
        registry
            .require(&ProviderKind::new("openai").expect("OAuth provider"))
            .is_err()
    );
    assert_eq!(
        ChannelAdminRegistry::new([Arc::clone(&provider), provider])
            .err()
            .expect("duplicate")
            .kind(),
        ProviderAdminErrorKind::Conflict
    );
    let channel = registry
        .require(&ProviderKind::new("openai_api").expect("API provider"))
        .expect("channel provider");
    let configured = channel.prepare_config(&document(json!({"baseUrl": "https://example.test/v1", "apiKey": "private-test-token", "models": ["configured-model"]})), None).expect("new configuration");
    let changed = channel
        .prepare_config(&document(json!({"project": "proj_new"})), Some(&configured))
        .expect("partial update");
    assert_eq!(changed.expose_to_provider()["apiKey"], "private-test-token");
    let public = channel.public_config(&changed).expect("public fields");
    let public = public.expose_to_provider().expose_to_provider();
    assert_eq!(public["project"], "proj_new");
    assert!(!public.contains_key("apiKey"));
    assert!(
        !serde_json::to_string(public)
            .expect("JSON")
            .contains("private-test-token")
    );
    let error = channel
        .prepare_config(
            &document(json!({"apiKey": "private-test-token\nheader"})),
            Some(&configured),
        )
        .expect_err("invalid replacement");
    assert_eq!(error.kind(), ProviderAdminErrorKind::Invalid);
    assert!(!format!("{error:?}").contains("private-test-token"));
}
