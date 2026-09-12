//! 外部渠道的中立持久化边界；连接地址、认证和协议字段由具体 Provider 解释。

use std::{fmt, num::NonZeroU64};

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    identity::{ChannelId, ProviderKind},
    provider_ports::ProviderStoreError,
    validation::IdentifierError,
};

/// 一个渠道配置的独立版本，阻止旧候选使用已经轮换的凭据。
/// 公共字段、启停和凭据的任意更新都会推进版本，防止管理端丢失更新。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelRevision(NonZeroU64);

impl ChannelRevision {
    pub fn new(value: u64) -> Result<Self, IdentifierError> {
        if value > i64::MAX as u64 {
            return Err(IdentifierError::InvalidFormat);
        }
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(IdentifierError::InvalidFormat)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// 一个渠道及其不可分割的配置版本，随目录、候选和调用事实传递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBinding {
    id: ChannelId,
    revision: ChannelRevision,
}

impl ChannelBinding {
    #[must_use]
    pub const fn new(id: ChannelId, revision: ChannelRevision) -> Self {
        Self { id, revision }
    }
    #[must_use]
    pub const fn id(&self) -> &ChannelId {
        &self.id
    }
    #[must_use]
    pub const fn revision(&self) -> ChannelRevision {
        self.revision
    }
}

/// Provider 校验后的不透明连接配置。普通列表、日志及 Debug 均不输出它。
#[derive(Clone, PartialEq)]
pub struct ProviderChannelConfig(Map<String, Value>);

impl ProviderChannelConfig {
    /// Core 只限制存储包络；URL、认证方式和字段语义必须由 Provider 校验。
    pub fn new(value: Map<String, Value>) -> Result<Self, IdentifierError> {
        if value.is_empty()
            || serde_json::to_vec(&value)
                .map_err(|_| IdentifierError::InvalidFormat)?
                .len()
                > 65_536
        {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn expose_to_provider(&self) -> &Map<String, Value> {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> Map<String, Value> {
        self.0
    }
}

impl fmt::Debug for ProviderChannelConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderChannelConfig(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredChannel {
    pub id: ChannelId,
    pub provider: ProviderKind,
    pub revision: ChannelRevision,
    pub config: ProviderChannelConfig,
}

/// Provider 读取真实渠道配置；调用时必须带快照冻结的连接版本。
pub trait ChannelStorePort: Send + Sync {
    fn load_channel<'a>(
        &'a self,
        id: &'a ChannelId,
        expected_revision: ChannelRevision,
    ) -> BoxFuture<'a, Result<Option<StoredChannel>, ProviderStoreError>>;

    /// 一个 Provider 的启用渠道，供其发现目录。数量超出实现边界时必须报错，不能静默截断。
    fn list_enabled_channels<'a>(
        &'a self,
        provider: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<Vec<StoredChannel>, ProviderStoreError>>;
}
