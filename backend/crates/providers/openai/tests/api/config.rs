use gateway_core::channel::ProviderChannelConfig;
use provider_openai::api::config::{ApiChannelConfig, ApiChannelConfigError};
use serde_json::{Value, json};

fn input(url: &str) -> Value {
    json!({"baseUrl": url, "apiKey": "test-sensitive-token", "models": ["configured-text-model"], "organization": "org_test", "project": "proj_test"})
}
fn parse(value: Value) -> Result<ApiChannelConfig, ApiChannelConfigError> {
    ApiChannelConfig::parse(
        &ProviderChannelConfig::new(value.as_object().expect("object").clone()).expect("envelope"),
    )
}

#[test]
fn responses_channel_preserves_custom_api_prefix_and_public_projection_has_no_secret() {
    for url in [
        "https://api.example.test/v1",
        "https://api.example.test/v1/",
    ] {
        let config = parse(input(url)).expect("valid config");
        assert_eq!(
            config.responses_url().as_str(),
            "https://api.example.test/v1/responses"
        );
        assert_eq!(
            config.models_url().as_str(),
            "https://api.example.test/v1/models"
        );
        assert_eq!(config.api_key(), "test-sensitive-token");
        assert_eq!(config.organization(), Some("org_test"));
        assert_eq!(config.project(), Some("proj_test"));
        assert_eq!(config.models().len(), 1);
        let public = config.public_config();
        assert_eq!(public["hasApiKey"], true);
        assert!(!public.contains_key("apiKey"));
        assert!(
            !serde_json::to_string(&public)
                .expect("public JSON")
                .contains("test-sensitive-token")
        );
        let stored = config.to_stored().expect("stored");
        assert_eq!(
            ApiChannelConfig::parse(&stored)
                .expect("round trip")
                .api_key(),
            "test-sensitive-token"
        );
    }
    let config =
        parse(input("http://127.0.0.1:1234/custom/api/")).expect("explicit private HTTP upstream");
    assert_eq!(
        config.responses_url().as_str(),
        "http://127.0.0.1:1234/custom/api/responses"
    );
}

#[test]
fn channel_rejects_embedded_secrets_header_injection_unknown_fields_and_ambiguous_models() {
    for url in [
        "https://secret@example.test/v1",
        "https://user:secret@example.test/v1",
        "https://example.test/v1?token=secret",
        "https://example.test/v1#secret",
        "file:///tmp/secret",
        "ftp://example.test/v1",
        "https://example.test/v1\n",
        " https://example.test/v1",
    ] {
        let error = parse(input(url)).err().expect("invalid endpoint");
        assert_eq!(error, ApiChannelConfigError::InvalidBaseUrl);
        assert!(!error.to_string().contains("secret"));
    }
    for (field, value, expected) in [
        (
            "apiKey",
            json!(""),
            ApiChannelConfigError::InvalidAuthentication,
        ),
        (
            "apiKey",
            json!("token\r\nInjected: yes"),
            ApiChannelConfigError::InvalidAuthentication,
        ),
        (
            "project",
            json!("proj\nsecret"),
            ApiChannelConfigError::InvalidAuthentication,
        ),
        ("models", json!([]), ApiChannelConfigError::InvalidModels),
        (
            "models",
            json!(["same", "same"]),
            ApiChannelConfigError::InvalidModels,
        ),
        (
            "models",
            json!([" padded"]),
            ApiChannelConfigError::InvalidModels,
        ),
        (
            "unknownSecret",
            json!("secret"),
            ApiChannelConfigError::InvalidFields,
        ),
    ] {
        let mut value_input = input("https://example.test/v1");
        value_input[field] = value;
        assert_eq!(parse(value_input).err().expect("invalid config"), expected);
    }
}

#[test]
fn partial_connection_edit_preserves_secret_but_explicit_invalid_replacement_is_rejected() {
    let current = parse(input("https://example.test/v1"))
        .expect("config")
        .to_stored()
        .expect("stored");
    let changed = ApiChannelConfig::prepare_update(
        &current,
        json!({"models": ["second-model"]})
            .as_object()
            .expect("patch")
            .clone(),
    )
    .expect("edit without secret");
    let changed = ApiChannelConfig::parse(&changed).expect("changed");
    assert_eq!(changed.api_key(), "test-sensitive-token");
    assert_eq!(
        changed.models().iter().next().expect("model").as_str(),
        "second-model"
    );
    for replacement in [json!(""), Value::Null, json!("malformed\nkey")] {
        assert!(
            ApiChannelConfig::prepare_update(
                &current,
                json!({"apiKey": replacement})
                    .as_object()
                    .expect("patch")
                    .clone()
            )
            .is_err()
        );
    }
    assert_eq!(
        ApiChannelConfig::parse(&current)
            .expect("original unchanged")
            .models()
            .iter()
            .next()
            .expect("model")
            .as_str(),
        "configured-text-model"
    );
}
