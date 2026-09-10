//! 下游接入分组独立于账号分组；模型授权在别名映射之前按对外名称匹配。

use std::{collections::BTreeSet, fmt};

use crate::account::scope::AccountGroupId;
use crate::identity::ChannelId;
use crate::validation::{IdentifierError, validate_text};

use super::RateLimits;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccessGroupId(String);

impl AccessGroupId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 128, false, Some("access_"))?;
        if value.len() == 7
            || !value[7..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccessGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 模型与来源均需显式授权；空集合不退化为全量授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupPolicy {
    pub id: AccessGroupId,
    pub enabled: bool,
    pub limits: RateLimits,
    pub allowed_models: BTreeSet<String>,
    pub pool_group_ids: BTreeSet<AccountGroupId>,
    pub channel_ids: BTreeSet<ChannelId>,
}

impl AccessGroupPolicy {
    pub fn validate(&self) -> Result<(), IdentifierError> {
        Self::validate_permissions(
            self.limits,
            &self.allowed_models,
            &self.pool_group_ids,
            &self.channel_ids,
        )
    }

    pub fn validate_permissions(
        limits: RateLimits,
        allowed_models: &BTreeSet<String>,
        pool_group_ids: &BTreeSet<AccountGroupId>,
        channel_ids: &BTreeSet<ChannelId>,
    ) -> Result<(), IdentifierError> {
        if allowed_models.len() > 2048
            || pool_group_ids.len() > 256
            || channel_ids.len() > 256
            || limits.max_concurrency > 9_007_199_254_740_991
            || limits.requests_per_minute > 9_007_199_254_740_991
        {
            return Err(IdentifierError::InvalidFormat);
        }
        for model in allowed_models {
            validate_text(model, 256, true, None)?;
            if model.trim() != model || model == "*" {
                return Err(IdentifierError::InvalidFormat);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn allows_model(&self, public_model: &str) -> bool {
        self.enabled && self.allowed_models.contains(public_model)
    }
}
