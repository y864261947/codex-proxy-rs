//! Responses 渠道连接配置的 Provider-owned 校验及脱敏投影。

use std::collections::BTreeSet;

use gateway_core::{channel::ProviderChannelConfig, routing::UpstreamModelId};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ApiChannelConfigError {
    #[error("invalid channel configuration fields")]
    InvalidFields,
    #[error("invalid channel base URL")]
    InvalidBaseUrl,
    #[error("invalid API authentication header")]
    InvalidAuthentication,
    #[error("invalid configured Responses models")]
    InvalidModels,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigWire {
    base_url: String,
    api_key: String,
    models: Vec<String>,
    organization: Option<String>,
    project: Option<String>,
}

/// 不提供 Debug/Serialize；只有明确的公共投影可进入管理响应。
pub struct ApiChannelConfig {
    base_url: Url,
    api_key: SecretString,
    models: BTreeSet<UpstreamModelId>,
    organization: Option<String>,
    project: Option<String>,
}

impl ApiChannelConfig {
    /// 编辑时未提交 API Key 就保留原值；空字符串和 null 不作为隐式清除指令。
    pub fn prepare_update(
        current: &ProviderChannelConfig,
        changes: Map<String, Value>,
    ) -> Result<ProviderChannelConfig, ApiChannelConfigError> {
        let mut merged = current.expose_to_provider().clone();
        merged.extend(changes);
        let merged =
            ProviderChannelConfig::new(merged).map_err(|_| ApiChannelConfigError::InvalidFields)?;
        Self::parse(&merged)?.to_stored()
    }

    pub fn parse(input: &ProviderChannelConfig) -> Result<Self, ApiChannelConfigError> {
        let wire: ConfigWire =
            serde_json::from_value(Value::Object(input.expose_to_provider().clone()))
                .map_err(|_| ApiChannelConfigError::InvalidFields)?;
        let mut base_url =
            Url::parse(&wire.base_url).map_err(|_| ApiChannelConfigError::InvalidBaseUrl)?;
        if wire.base_url.len() > 2048
            || wire.base_url.trim() != wire.base_url
            || wire.base_url.chars().any(char::is_control)
            || !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(ApiChannelConfigError::InvalidBaseUrl);
        }
        // 配置地址包含上游 API 前缀，例如 /v1；统一尾斜杠后拼接相对端点。
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        if wire.api_key.is_empty()
            || wire.api_key.len() > 16_384
            || !wire
                .api_key
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte))
            || [&wire.organization, &wire.project]
                .into_iter()
                .flatten()
                .any(|value| {
                    value.is_empty()
                        || value.len() > 256
                        || !value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
        {
            return Err(ApiChannelConfigError::InvalidAuthentication);
        }
        if wire.models.is_empty() || wire.models.len() > 1_000 {
            return Err(ApiChannelConfigError::InvalidModels);
        }
        let mut models = BTreeSet::new();
        for model in wire.models {
            if model.trim() != model {
                return Err(ApiChannelConfigError::InvalidModels);
            }
            let model =
                UpstreamModelId::new(model).map_err(|_| ApiChannelConfigError::InvalidModels)?;
            if !models.insert(model) {
                return Err(ApiChannelConfigError::InvalidModels);
            }
        }
        Ok(Self {
            base_url,
            api_key: SecretString::from(wire.api_key),
            models,
            organization: wire.organization,
            project: wire.project,
        })
    }

    #[must_use]
    pub fn public_config(&self) -> Map<String, Value> {
        Map::from_iter([
            ("baseUrl".to_owned(), json!(self.base_url.as_str())),
            (
                "models".to_owned(),
                json!(
                    self.models
                        .iter()
                        .map(UpstreamModelId::as_str)
                        .collect::<Vec<_>>()
                ),
            ),
            ("organization".to_owned(), json!(self.organization)),
            ("project".to_owned(), json!(self.project)),
            ("hasApiKey".to_owned(), Value::Bool(true)),
        ])
    }

    pub fn to_stored(&self) -> Result<ProviderChannelConfig, ApiChannelConfigError> {
        let mut config = self.public_config();
        config.remove("hasApiKey");
        config.insert(
            "apiKey".to_owned(),
            Value::String(self.api_key.expose_secret().to_owned()),
        );
        ProviderChannelConfig::new(config).map_err(|_| ApiChannelConfigError::InvalidFields)
    }

    #[must_use]
    pub fn responses_url(&self) -> Url {
        self.base_url
            .join("responses")
            .expect("validated hierarchical base URL")
    }

    #[must_use]
    pub fn models_url(&self) -> Url {
        self.base_url
            .join("models")
            .expect("validated hierarchical base URL")
    }

    #[must_use]
    pub const fn models(&self) -> &BTreeSet<UpstreamModelId> {
        &self.models
    }

    /// 仅 transport 构造请求头时借用；不能写入错误、审计或公共配置。
    #[must_use]
    pub fn api_key(&self) -> &str {
        self.api_key.expose_secret()
    }

    #[must_use]
    pub fn organization(&self) -> Option<&str> {
        self.organization.as_deref()
    }

    #[must_use]
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
}
