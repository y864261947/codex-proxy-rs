//! 渠道公共配置与 Provider 校验后的连接配置分别持久化和投影。

use chrono::{DateTime, Utc};
use gateway_core::{
    channel::{ChannelRevision, ProviderChannelConfig},
    identity::{ChannelId, ProviderKind, QuotaScopeId},
    policy::RateLimits,
    routing::source::{SourceId, SourcePolicy, SourcePreference},
};
use std::collections::BTreeSet;

use super::provider_credentials::ProviderDocument;
use super::{AdminError, PageSize, Revision};

#[derive(Debug, Clone)]
pub struct DiscoveredChannelModels {
    pub configured: BTreeSet<gateway_core::routing::UpstreamModelId>,
    pub discovered: BTreeSet<gateway_core::routing::UpstreamModelId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelModelPreview {
    pub id: ChannelId,
    pub revision: ChannelRevision,
    pub generation: u64,
    pub fetched_at: DateTime<Utc>,
    pub added: Vec<String>,
    pub missing: Vec<String>,
    pub unchanged: Vec<String>,
}

impl ChannelModelPreview {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.generation == 0
            || self.generation > i64::MAX as u64
            || self.added.len() + self.unchanged.len() > 1000
            || !(1..=1000).contains(&(self.missing.len() + self.unchanged.len()))
        {
            return Err(AdminError::invalid("发现快照数量或顺序不合法"));
        }
        let mut seen = BTreeSet::new();
        for values in [&self.added, &self.missing, &self.unchanged] {
            if values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(AdminError::invalid("发现快照必须有序且不重复"));
            }
            for value in values {
                if value.trim() != value || !seen.insert(value) {
                    return Err(AdminError::invalid("发现快照模型集合不合法"));
                }
                gateway_core::routing::UpstreamModelId::new(value.clone())
                    .map_err(|_| AdminError::invalid("发现快照模型 ID 不合法"))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NewChannel {
    pub provider: ProviderKind,
    pub fields: ChannelFields,
    pub config: ProviderDocument,
}

#[derive(Debug, Clone)]
pub struct ChannelDiscoveryQuery {
    pub id: ChannelId,
    pub before_generation: Option<u64>,
    pub page_size: PageSize,
}

impl ChannelDiscoveryQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page_size.get() > 50
            || self
                .before_generation
                .is_some_and(|value| value == 0 || value > i64::MAX as u64)
        {
            return Err(AdminError::invalid("发现历史分页参数不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ChannelDiscoveryPage {
    pub items: Vec<ChannelModelPreview>,
    pub next_before_generation: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ChannelDiscoveryComparisonQuery {
    pub id: ChannelId,
    pub base_generation: u64,
    pub target_generation: u64,
}

impl ChannelDiscoveryComparisonQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.base_generation == 0
            || self.base_generation >= self.target_generation
            || self.target_generation > i64::MAX as u64
        {
            return Err(AdminError::invalid(
                "请选择同一渠道中序号递增的两条不同记录",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ChannelDiscoveryPair {
    pub base: ChannelModelPreview,
    pub target: ChannelModelPreview,
}

#[derive(Debug, Clone)]
pub struct ChannelDiscoveryComparison {
    pub records: ChannelDiscoveryPair,
    pub appeared: Vec<String>,
    pub disappeared: Vec<String>,
    pub unchanged: Vec<String>,
}

impl ChannelDiscoveryPair {
    pub fn compare(self) -> Result<ChannelDiscoveryComparison, AdminError> {
        self.base.validate()?;
        self.target.validate()?;
        if self.base.id != self.target.id {
            return Err(AdminError::invalid("不能比较不同渠道的发现记录"));
        }
        ChannelDiscoveryComparisonQuery {
            id: self.base.id.clone(),
            base_generation: self.base.generation,
            target_generation: self.target.generation,
        }
        .validate()?;
        let base: BTreeSet<_> = self.base.added.iter().chain(&self.base.unchanged).collect();
        let target: BTreeSet<_> = self
            .target
            .added
            .iter()
            .chain(&self.target.unchanged)
            .collect();
        let appeared = target
            .difference(&base)
            .map(|value| value.to_string())
            .collect();
        let disappeared = base
            .difference(&target)
            .map(|value| value.to_string())
            .collect();
        let unchanged = base
            .intersection(&target)
            .map(|value| value.to_string())
            .collect();
        Ok(ChannelDiscoveryComparison {
            records: self,
            appeared,
            disappeared,
            unchanged,
        })
    }
}

#[derive(Debug, Clone)]
pub struct UpdateChannel {
    pub id: ChannelId,
    pub expected_revision: ChannelRevision,
    pub fields: ChannelFields,
    pub config: Option<ProviderDocument>,
}

/// Provider 显式脱敏后的编辑投影，版本随投影返回以避免覆盖更新。
#[derive(Debug, Clone)]
pub struct ChannelConnection {
    pub id: ChannelId,
    pub provider: ProviderKind,
    pub revision: ChannelRevision,
    pub config: ProviderDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMutation {
    pub id: ChannelId,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelFields {
    pub name: String,
    pub note: Option<String>,
    pub enabled: bool,
    pub preference: SourcePreference,
    pub limits: RateLimits,
    pub quota_scope_id: Option<QuotaScopeId>,
}

impl ChannelFields {
    pub fn validate(&self, id: &ChannelId) -> Result<(), AdminError> {
        SourcePolicy::new(
            SourceId::Channel(id.clone()),
            self.enabled,
            self.preference,
            self.limits,
            self.quota_scope_id.clone(),
        )
        .and_then(|policy| policy.with_name(self.name.clone()))
        .map_err(|_| AdminError::invalid("渠道名称或限额不合法"))?;
        if self.note.as_deref().is_some_and(|note| {
            note.chars().count() > 1024 || note.chars().any(|c| c.is_control() && c != '\n')
        }) {
            return Err(AdminError::invalid("渠道备注不合法"));
        }
        Ok(())
    }
}

/// 此投影没有凭据字段，避免普通管理读取意外获得完整 API Key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRecord {
    pub id: ChannelId,
    pub provider: ProviderKind,
    pub fields: ChannelFields,
    pub connection_revision: ChannelRevision,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: Option<String>,
    pub provider: Option<ProviderKind>,
}

impl ChannelListQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self
                .search
                .as_deref()
                .is_some_and(|search| search.len() > 256 || search.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("渠道查询条件不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPage {
    pub items: Vec<ChannelRecord>,
    pub total: u64,
    pub config_revision: Revision,
}

/// 只有 Provider 校验后的连接配置能进入本提交命令；原始 HTTP 输入不是此类型。
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelChange {
    Create {
        id: ChannelId,
        provider: ProviderKind,
        fields: ChannelFields,
        config: ProviderChannelConfig,
    },
    Update {
        id: ChannelId,
        expected_revision: ChannelRevision,
        fields: ChannelFields,
        replacement_config: Option<ProviderChannelConfig>,
    },
    Delete {
        id: ChannelId,
        expected_revision: ChannelRevision,
    },
}

impl ChannelChange {
    #[must_use]
    pub const fn id(&self) -> &ChannelId {
        match self {
            Self::Create { id, .. } | Self::Update { id, .. } | Self::Delete { id, .. } => id,
        }
    }

    pub fn validate(&self) -> Result<(), AdminError> {
        match self {
            Self::Create { id, fields, .. } | Self::Update { id, fields, .. } => {
                fields.validate(id)
            }
            Self::Delete { .. } => Ok(()),
        }
    }
}
