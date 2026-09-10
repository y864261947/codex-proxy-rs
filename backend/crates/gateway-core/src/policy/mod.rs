//! 下游 Client API Key 的准入策略。
//!
//! 旧 Client API Key 保留账号分组权限；接入分组同时约束模型与可用号池。

mod access;
mod admission;
mod client_version;

pub use access::{AccessGroupId, AccessGroupPolicy};
pub use admission::{AdmissionScope, AdmissionScopeId, CustomerId, CustomerPolicy};

pub use client_version::{
    ClientVersionRejection, CodexClientKind, CodexClientMinVersions, CodexClientVersion,
};

use std::fmt;
use std::sync::Arc;

use crate::account::scope::FrozenAccountScope;
use crate::validation::{IdentifierError, PolicyError, validate_text};

/// `client_api_keys.id` 的核心值对象。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientApiKeyId(String);

impl ClientApiKeyId {
    /// 校验并创建 Key ID。
    ///
    /// # Errors
    ///
    /// ID 为空、过长或包含控制字符时返回错误。
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 128, false, None)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientApiKeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// RuntimeSnapshot 中用于同步认证的明文 Client API Key。
///
/// 数据库按产品约束明文保存；该值对象只负责阻止 `Debug`/日志意外输出。
#[derive(Clone, PartialEq, Eq)]
pub struct PlaintextClientApiKey(String);

impl PlaintextClientApiKey {
    /// 校验并创建明文 Key。
    ///
    /// # Errors
    ///
    /// Key 为空、过长或包含控制字符时返回错误。
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_text(&value, 512, false, None)?;
        Ok(Self(value))
    }

    /// 仅借给同步认证器做常量时间比较。
    #[must_use]
    pub fn expose_for_auth(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PlaintextClientApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlaintextClientApiKey(<redacted>)")
    }
}

/// 零表示对应维度不额外限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RateLimits {
    pub max_concurrency: u64,
    pub requests_per_minute: u64,
}

impl RateLimits {
    /// 控制台和协调存储共同支持的精确非负整数范围。
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.max_concurrency <= 9_007_199_254_740_991
            && self.requests_per_minute <= 9_007_199_254_740_991
    }

    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_concurrency: 0,
            requests_per_minute: 0,
        }
    }
}

/// 从 `client_api_keys` 冻结的公开准入事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPolicy {
    global_limits: RateLimits,
    customer: Option<CustomerPolicy>,
    access_group: Option<AccessGroupPolicy>,
    key_id: ClientApiKeyId,
    plaintext_key: PlaintextClientApiKey,
    account_scope: Arc<FrozenAccountScope>,
    enabled: bool,
    limits: RateLimits,
}

impl ClientPolicy {
    #[must_use]
    pub const fn new(
        key_id: ClientApiKeyId,
        plaintext_key: PlaintextClientApiKey,
        account_scope: Arc<FrozenAccountScope>,
        enabled: bool,
        limits: RateLimits,
    ) -> Self {
        Self {
            key_id,
            global_limits: RateLimits::unlimited(),
            customer: None,
            access_group: None,
            plaintext_key,
            account_scope,
            enabled,
            limits,
        }
    }

    #[must_use]
    pub const fn key_id(&self) -> &ClientApiKeyId {
        &self.key_id
    }

    #[must_use]
    pub const fn with_global_limits(mut self, limits: RateLimits) -> Self {
        self.global_limits = limits;
        self
    }

    #[must_use]
    pub fn with_customer(mut self, customer: Option<CustomerPolicy>) -> Self {
        self.customer = customer;
        self
    }

    #[must_use]
    pub const fn customer(&self) -> Option<&CustomerPolicy> {
        self.customer.as_ref()
    }

    #[must_use]
    pub fn with_access_group(mut self, access_group: Option<AccessGroupPolicy>) -> Self {
        self.access_group = access_group;
        self
    }

    #[must_use]
    pub const fn access_group(&self) -> Option<&AccessGroupPolicy> {
        self.access_group.as_ref()
    }

    /// 未迁入接入分组的旧 Key 继续使用原有模型可见性。
    #[must_use]
    pub fn allows_model(&self, public_model: &str) -> bool {
        self.enabled()
            && self
                .access_group
                .as_ref()
                .is_none_or(|group| group.allows_model(public_model))
    }

    #[must_use]
    pub fn admission_scopes(&self) -> Vec<AdmissionScope> {
        let mut scopes = vec![AdmissionScope {
            id: AdmissionScopeId::Key(self.key_id.clone()),
            limits: self.limits,
        }];
        if let Some(customer) = &self.customer {
            scopes.push(AdmissionScope {
                id: AdmissionScopeId::Customer(customer.id.clone()),
                limits: customer.limits,
            });
        }
        if let Some(group) = &self.access_group {
            scopes.push(AdmissionScope {
                id: AdmissionScopeId::AccessGroup(group.id.clone()),
                limits: group.limits,
            });
        }
        // 即使当前不限，也保留全局占用；调小上限后不能忽略已运行的请求。
        scopes.push(AdmissionScope {
            id: AdmissionScopeId::Global,
            limits: self.global_limits,
        });
        scopes
    }

    #[must_use]
    pub const fn plaintext_key(&self) -> &PlaintextClientApiKey {
        &self.plaintext_key
    }

    #[must_use]
    pub const fn account_scope(&self) -> &Arc<FrozenAccountScope> {
        &self.account_scope
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
            && match &self.customer {
                Some(customer) => customer.enabled,
                None => true,
            }
            && match &self.access_group {
                Some(group) => group.enabled,
                None => true,
            }
    }

    #[must_use]
    pub const fn limits(&self) -> RateLimits {
        self.limits
    }

    /// 禁用的 Key 不接受新请求。
    ///
    /// # Errors
    ///
    /// Key 已禁用时返回稳定拒绝原因。
    pub fn authorize(&self) -> Result<(), PolicyError> {
        if self
            .access_group
            .as_ref()
            .is_some_and(|group| !group.enabled)
        {
            return Err(PolicyError::Denied {
                reason: "access group is disabled",
            });
        }
        if self
            .customer
            .as_ref()
            .is_some_and(|customer| !customer.enabled)
        {
            return Err(PolicyError::Denied {
                reason: "customer is disabled",
            });
        }
        if self.enabled {
            Ok(())
        } else {
            Err(PolicyError::Denied {
                reason: "client API key is disabled",
            })
        }
    }
}
