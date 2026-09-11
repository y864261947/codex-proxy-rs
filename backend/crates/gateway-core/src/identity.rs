//! 编译期 Provider 身份；账号与路由共同使用，不依赖路由计划。

use crate::validation::{IdentifierError, validate_text};
use std::fmt;

/// 编译进二进制的 Provider adapter slug。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderKind(String);

impl ProviderKind {
    /// 校验 Provider slug。
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 64, true, None)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 一个实际外部上游渠道；改名、换凭据不改变该身份。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_scoped_identity(&value, "chan_")?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 管理员显式关联的共享上游配额，不由渠道名称或凭据值推导。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuotaScopeId(String);

impl QuotaScopeId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_scoped_identity(&value, "quota_")?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QuotaScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn validate_scoped_identity(value: &str, prefix: &'static str) -> Result<(), IdentifierError> {
    validate_text(value, 128, false, Some(prefix))?;
    if value.len() == prefix.len()
        || !value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(IdentifierError::InvalidFormat);
    }
    Ok(())
}

/// `account_groups.id` 的核心值对象。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountGroupId(String);

impl AccountGroupId {
    /// 校验并创建账号分组 ID。
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("grp_") else {
            return Err(IdentifierError::MissingPrefix { expected: "grp_" });
        };
        if suffix.len() != 32
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

impl fmt::Display for AccountGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

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

impl SourceId {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AccountPool(_) => "pool",
            Self::Channel(_) => "channel",
        }
    }

    #[must_use]
    pub fn reference(&self) -> &str {
        match self {
            Self::AccountPool(id) => id.as_str(),
            Self::Channel(id) => id.as_str(),
        }
    }
}
