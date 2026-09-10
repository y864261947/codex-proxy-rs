//! 共享上游项目的具名配额，不包含凭据和下游客户身份。

use chrono::{DateTime, Utc};
use gateway_core::{identity::QuotaScopeId, policy::RateLimits};

use super::{AdminError, PageSize, Revision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopeFields {
    pub name: String,
    pub note: Option<String>,
    pub enabled: bool,
    pub limits: RateLimits,
}

impl QuotaScopeFields {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.name.is_empty()
            || self.name.trim() != self.name
            || self.name.chars().count() > 128
            || self.name.chars().any(char::is_control)
            || self.note.as_deref().is_some_and(|note| {
                note.chars().count() > 1024 || note.chars().any(|c| c.is_control() && c != '\n')
            })
            || self.limits.max_concurrency > (1_u64 << 53) - 1
            || self.limits.requests_per_minute > (1_u64 << 53) - 1
        {
            return Err(AdminError::invalid("共享配额名称、备注或限额不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopeRecord {
    pub id: QuotaScopeId,
    pub fields: QuotaScopeFields,
    pub source_count: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopeListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: Option<String>,
}

impl QuotaScopeListQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self
                .search
                .as_deref()
                .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("共享配额查询条件不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopePage {
    pub items: Vec<QuotaScopeRecord>,
    pub total: u64,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaScopeChange {
    Create {
        id: QuotaScopeId,
        fields: QuotaScopeFields,
    },
    Update {
        id: QuotaScopeId,
        fields: QuotaScopeFields,
    },
    Delete {
        id: QuotaScopeId,
    },
}

impl QuotaScopeChange {
    #[must_use]
    pub fn entity_ref(&self) -> &str {
        match self {
            Self::Create { id, .. } | Self::Update { id, .. } | Self::Delete { id } => id.as_str(),
        }
    }

    pub fn validate(&self) -> Result<(), AdminError> {
        match self {
            Self::Create { fields, .. } | Self::Update { fields, .. } => fields.validate(),
            Self::Delete { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopeMutation {
    pub id: String,
    pub config_revision: Revision,
}
