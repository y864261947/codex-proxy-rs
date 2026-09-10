//! 下游限额的身份与冻结策略；每个范围独立累计，同一次准入必须全部通过。

use std::fmt;

use crate::validation::{IdentifierError, validate_text};

use super::{ClientApiKeyId, RateLimits};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CustomerId(String);

impl CustomerId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 128, false, Some("cust_"))?;
        if value.len() == 5
            || !value[5..]
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

impl fmt::Display for CustomerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerPolicy {
    pub id: CustomerId,
    pub enabled: bool,
    pub limits: RateLimits,
}

/// 类型参与身份，避免不同层级的同名 ID 共用计数。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdmissionScopeId {
    Key(ClientApiKeyId),
    Customer(CustomerId),
}

impl fmt::Display for AdmissionScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(id) => write!(formatter, "key:{id}"),
            Self::Customer(id) => write!(formatter, "customer:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionScope {
    pub id: AdmissionScopeId,
    pub limits: RateLimits,
}
