//! API 渠道的管理配置校验，不包含账号导入或 Codex OAuth 行为。

use gateway_admin::{
    model::provider_credentials::ProviderDocument,
    ports::{
        channels::ChannelProviderAdmin,
        provider::{ProviderAdminError, ProviderAdminErrorKind},
    },
};
use gateway_core::{
    account::OpaqueProviderData, channel::ProviderChannelConfig, identity::ProviderKind,
};

use super::config::ApiChannelConfig;

pub struct ApiChannelAdmin {
    provider: ProviderKind,
}

impl Default for ApiChannelAdmin {
    fn default() -> Self {
        Self {
            provider: ProviderKind::new("openai_api").expect("static API adapter identity"),
        }
    }
}

impl ChannelProviderAdmin for ApiChannelAdmin {
    fn discover_models<'a>(
        &'a self,
        config: &'a ProviderChannelConfig,
    ) -> futures::future::BoxFuture<
        'a,
        Result<gateway_admin::model::channels::DiscoveredChannelModels, ProviderAdminError>,
    > {
        Box::pin(async move {
            let config = ApiChannelConfig::parse(config).map_err(|_| invalid())?;
            super::discovery::discover(config).await
        })
    }

    fn provider_kind(&self) -> &ProviderKind {
        &self.provider
    }

    fn prepare_config(
        &self,
        input: &ProviderDocument,
        current: Option<&ProviderChannelConfig>,
    ) -> Result<ProviderChannelConfig, ProviderAdminError> {
        let input = input.expose_to_provider().expose_to_provider().clone();
        match current {
            Some(current) => {
                ApiChannelConfig::prepare_update(current, input).map_err(|_| invalid())
            }
            None => {
                let input = ProviderChannelConfig::new(input).map_err(|_| invalid())?;
                ApiChannelConfig::parse(&input)
                    .map_err(|_| invalid())?
                    .to_stored()
                    .map_err(|_| invalid())
            }
        }
    }

    fn public_config(
        &self,
        config: &ProviderChannelConfig,
    ) -> Result<ProviderDocument, ProviderAdminError> {
        let config = ApiChannelConfig::parse(config).map_err(|_| invalid())?;
        Ok(ProviderDocument::new(OpaqueProviderData::new(
            config.public_config(),
        )))
    }
}

fn invalid() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid)
}
