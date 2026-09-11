//! 渠道连接配置由协议 Provider 校验；通用管理用例不解释 URL 或凭据。

use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;
use gateway_core::{channel::ProviderChannelConfig, identity::ProviderKind};

use super::provider::{ProviderAdminError, ProviderAdminErrorKind};
use crate::model::provider_credentials::ProviderDocument;

pub trait ChannelProviderAdmin: Send + Sync {
    fn discover_models<'a>(
        &'a self,
        _config: &'a ProviderChannelConfig,
    ) -> BoxFuture<'a, Result<crate::model::channels::DiscoveredChannelModels, ProviderAdminError>>
    {
        Box::pin(async { Err(ProviderAdminError::new(ProviderAdminErrorKind::Unsupported)) })
    }

    fn provider_kind(&self) -> &ProviderKind;

    /// 当前配置仅用于更新合并；None 表示必须提交完整新连接配置。
    fn prepare_config(
        &self,
        input: &ProviderDocument,
        current: Option<&ProviderChannelConfig>,
    ) -> Result<ProviderChannelConfig, ProviderAdminError>;

    /// 显式构造无凭据的公共投影，禁止直接回传持久配置。
    fn public_config(
        &self,
        config: &ProviderChannelConfig,
    ) -> Result<ProviderDocument, ProviderAdminError>;
}

#[derive(Clone)]
pub struct ChannelAdminRegistry {
    providers: Arc<BTreeMap<ProviderKind, Arc<dyn ChannelProviderAdmin>>>,
}

impl ChannelAdminRegistry {
    pub fn new(
        providers: impl IntoIterator<Item = Arc<dyn ChannelProviderAdmin>>,
    ) -> Result<Self, ProviderAdminError> {
        let mut registered = BTreeMap::new();
        for provider in providers {
            if registered
                .insert(provider.provider_kind().clone(), provider)
                .is_some()
            {
                return Err(ProviderAdminError::new(ProviderAdminErrorKind::Conflict));
            }
        }
        Ok(Self {
            providers: Arc::new(registered),
        })
    }

    pub fn require(
        &self,
        provider: &ProviderKind,
    ) -> Result<Arc<dyn ChannelProviderAdmin>, ProviderAdminError> {
        self.providers
            .get(provider)
            .cloned()
            .ok_or_else(|| ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))
    }

    pub fn kinds(&self) -> impl Iterator<Item = &ProviderKind> {
        self.providers.keys()
    }
}
