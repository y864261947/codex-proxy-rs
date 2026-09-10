//! 每次调用固定一个渠道与配置版本的 Responses API Provider。

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use gateway_core::{
    channel::{ChannelBinding, ChannelStorePort},
    engine::{
        AttemptContext,
        provider::{
            Provider, ProviderCallMetadata, ProviderCatalogGeneration, ProviderModelCapabilities,
            ProviderRequest, ProviderStream,
        },
    },
    error::{ProviderError, ProviderErrorKind},
    identity::ProviderKind,
    operation::{Operation, OperationKind},
    routing::{ModelCapabilities, ModelPresentation},
    upstream::{UpstreamSendState, UpstreamTransport},
};
use reqwest::Client;
use serde_json::Value;

use super::{config::ApiChannelConfig, stream::response_stream};

pub const PROVIDER_NAME: &str = "openai_api";

/// 只持有渠道存储和无默认认证的 HTTP Client；凭据按冻结版本读取。
pub struct ApiChannelProvider {
    store: Arc<dyn ChannelStorePort>,
    client: Client,
    generation: AtomicU64,
    catalog_versions: Mutex<BTreeMap<String, u64>>,
}

impl ApiChannelProvider {
    pub fn new(store: Arc<dyn ChannelStorePort>) -> Result<Self, ProviderError> {
        // 复用同一 crate 的 TLS / CA 配置；Client 禁止重定向且不包含 OAuth 请求头。
        let client = crate::transport::build_reqwest_client().map_err(|_| infrastructure())?;
        Ok(Self {
            store,
            client,
            generation: AtomicU64::new(0),
            catalog_versions: Mutex::new(BTreeMap::new()),
        })
    }
}

#[async_trait]
impl Provider for ApiChannelProvider {
    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(self.generation.load(Ordering::Acquire))
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        let provider = provider_kind();
        let channels = self
            .store
            .list_enabled_channels(&provider)
            .await
            .map_err(|_| infrastructure())?;
        let mut models = Vec::new();
        let mut versions = BTreeMap::new();
        for channel in channels {
            if channel.provider != provider
                || versions
                    .insert(channel.id.as_str().to_owned(), channel.revision.get())
                    .is_some()
            {
                return Err(infrastructure());
            }
            let config = ApiChannelConfig::parse(&channel.config).map_err(|_| infrastructure())?;
            let binding = ChannelBinding::new(channel.id, channel.revision);
            for model in config.models() {
                models.push(
                    ProviderModelCapabilities::new(
                        model.clone(),
                        ModelCapabilities::new([OperationKind::Generate].into(), None)
                            .with_upstream_feature_validation(),
                    )
                    .with_channel(binding.clone())
                    .with_presentation(ModelPresentation::new(
                        Some(model.as_str().to_owned()),
                        None,
                    )),
                );
            }
        }
        let mut current = self
            .catalog_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current != versions {
            *current = versions;
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(models)
    }

    async fn execute(
        &self,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let candidate = request.candidate();
        let invalid = || {
            ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
            )
        };
        if candidate.provider() != &provider_kind() || context.required_account().is_some() {
            return Err(invalid());
        }
        let binding = candidate.channel_binding().cloned().ok_or_else(invalid)?;
        let model = candidate.upstream_model().cloned().ok_or_else(invalid)?;
        let Operation::Generate(generate) = request.operation() else {
            return Err(invalid());
        };
        if generate.protocol_payload().protocol() != "openai" {
            return Err(invalid());
        }
        let body = generate.protocol_payload().body();
        // 续接必须先建立渠道级 owner 合同；禁止把别处创建的 response/conversation 交给随机渠道。
        if context.continuation().is_some()
            || generate.native_continuation_requested()
            || body
                .get("conversation")
                .is_some_and(|value| !value.is_null())
        {
            return Err(ProviderError::new(
                ProviderErrorKind::ContinuationRecoveryRequired,
                UpstreamSendState::NotSent,
            ));
        }
        // 异步任务要占用独立任务租约；同步转发不能提前返回 queued 后释放容量。
        if body
            .get("background")
            .is_some_and(|value| value != &Value::Bool(false) && !value.is_null())
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Unsupported,
                UpstreamSendState::NotSent,
            ));
        }
        let stored = self
            .store
            .load_channel(binding.id(), binding.revision())
            .await
            .map_err(|_| infrastructure())?
            .ok_or_else(|| {
                ProviderError::new(ProviderErrorKind::Unavailable, UpstreamSendState::NotSent)
            })?;
        if stored.id != *binding.id()
            || stored.revision != binding.revision()
            || stored.provider != *candidate.provider()
        {
            return Err(infrastructure());
        }
        let config = ApiChannelConfig::parse(&stored.config).map_err(|_| infrastructure())?;
        if !config.models().contains(&model) {
            return Err(invalid());
        }
        let mut body = body.clone();
        body.insert("model".to_owned(), Value::String(model.as_str().to_owned()));
        // HTTP JSON 和客户端 WS 均由 API 层消费同一 Responses SSE 协议事实。
        body.insert("stream".to_owned(), Value::Bool(true));
        let metadata = ProviderCallMetadata::for_channel(
            provider_kind(),
            Some(model.clone()),
            binding,
            transport(),
        );
        let events = response_stream(self.client.clone(), config, body, model, context);
        Ok(ProviderStream::new(metadata, events, ()))
    }
}

pub(super) fn provider_kind() -> ProviderKind {
    ProviderKind::new(PROVIDER_NAME).expect("static provider")
}
pub(super) fn transport() -> UpstreamTransport {
    UpstreamTransport::new("http_sse").expect("static transport")
}
fn infrastructure() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::ProviderInfrastructureUnavailable,
        UpstreamSendState::NotSent,
    )
}
