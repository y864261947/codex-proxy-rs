//! 来源身份与选择策略；不包含凭据、URL 或具体上游协议。

use std::{collections::BTreeSet, fmt, num::NonZeroU16};

use crate::{
    account::scope::AccountGroupId,
    identity::{ChannelId, QuotaScopeId},
    policy::RateLimits,
    validation::IdentifierError,
};

/// 一次上游尝试所属的实际来源。账号的真实身份独立保存。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    AccountPool(AccountGroupId),
    Channel(ChannelId),
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccountPool(id) => write!(formatter, "pool:{id}"),
            Self::Channel(id) => write!(formatter, "channel:{id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePreference {
    priority: NonZeroU16,
    weight: NonZeroU16,
}

impl SourcePreference {
    pub fn new(priority: u16, weight: u16) -> Result<Self, IdentifierError> {
        Ok(Self {
            priority: NonZeroU16::new(priority).ok_or(IdentifierError::InvalidFormat)?,
            weight: NonZeroU16::new(weight).ok_or(IdentifierError::InvalidFormat)?,
        })
    }

    #[must_use]
    pub const fn priority(self) -> u16 {
        self.priority.get()
    }

    #[must_use]
    pub const fn weight(self) -> u16 {
        self.weight.get()
    }
}

impl Default for SourcePreference {
    fn default() -> Self {
        Self {
            priority: NonZeroU16::MIN,
            weight: NonZeroU16::MIN,
        }
    }
}

/// 来源全局容量与默认选择偏好。组内覆盖只能改变偏好，不能改容量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePolicy {
    id: SourceId,
    enabled: bool,
    preference: SourcePreference,
    limits: RateLimits,
    quota_scope_id: Option<QuotaScopeId>,
}

impl SourcePolicy {
    pub fn new(
        id: SourceId,
        enabled: bool,
        preference: SourcePreference,
        limits: RateLimits,
        quota_scope_id: Option<QuotaScopeId>,
    ) -> Result<Self, IdentifierError> {
        if !limits.is_valid() {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self {
            id,
            enabled,
            preference,
            limits,
            quota_scope_id,
        })
    }

    #[must_use]
    pub const fn id(&self) -> &SourceId {
        &self.id
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn limits(&self) -> RateLimits {
        self.limits
    }

    #[must_use]
    pub const fn quota_scope_id(&self) -> Option<&QuotaScopeId> {
        self.quota_scope_id.as_ref()
    }

    #[must_use]
    pub fn effective_preference(
        &self,
        group_override: Option<SourcePreference>,
    ) -> SourcePreference {
        group_override.unwrap_or(self.preference)
    }
}

/// 去重与关闭空集合语义在 Core 保持一致，不以未选择表示全部来源。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedSources(BTreeSet<SourceId>);

impl AllowedSources {
    #[must_use]
    pub fn new(sources: impl IntoIterator<Item = SourceId>) -> Self {
        Self(sources.into_iter().collect())
    }

    #[must_use]
    pub fn allows(&self, source: &SourceId) -> bool {
        self.0.contains(source)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SourceId> {
        self.0.iter()
    }
}
