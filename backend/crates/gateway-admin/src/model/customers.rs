//! 下游客户仅用于归属和共享限额，不包含登录身份。

use chrono::{DateTime, Utc};
use gateway_core::policy::{ClientApiKeyId, CustomerId, RateLimits};

use super::{AdminError, PageSize, Revision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerFields {
    pub name: String,
    pub note: Option<String>,
    pub enabled: bool,
    pub limits: RateLimits,
}

impl CustomerFields {
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
            return Err(AdminError::invalid("客户名称、备注或限额不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerRecord {
    pub id: CustomerId,
    pub fields: CustomerFields,
    pub key_count: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerRef {
    pub id: CustomerId,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: Option<String>,
}

impl CustomerListQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self
                .search
                .as_deref()
                .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("客户查询条件不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerPage {
    pub items: Vec<CustomerRecord>,
    pub total: u64,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomerChange {
    Create {
        id: CustomerId,
        fields: CustomerFields,
    },
    Update {
        id: CustomerId,
        fields: CustomerFields,
    },
    Delete {
        id: CustomerId,
    },
    AssignKey {
        key_id: ClientApiKeyId,
        customer_id: Option<CustomerId>,
    },
}

impl CustomerChange {
    #[must_use]
    pub fn entity_ref(&self) -> &str {
        match self {
            Self::Create { id, .. } | Self::Update { id, .. } | Self::Delete { id } => id.as_str(),
            Self::AssignKey { key_id, .. } => key_id.as_str(),
        }
    }

    pub fn validate(&self) -> Result<(), AdminError> {
        match self {
            Self::Create { fields, .. } | Self::Update { fields, .. } => fields.validate(),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerMutation {
    pub id: String,
    pub config_revision: Revision,
}
