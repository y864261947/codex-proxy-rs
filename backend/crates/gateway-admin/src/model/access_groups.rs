//! 下游接入分组定义模型白名单、来源范围与共享限额。

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use gateway_core::{
    account::scope::AccountGroupId,
    policy::{AccessGroupId, AccessGroupPolicy, ClientApiKeyId, RateLimits},
};

use super::{AdminError, PageSize, Revision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupFields {
    pub name: String,
    pub note: Option<String>,
    pub enabled: bool,
    pub limits: RateLimits,
    pub allowed_models: BTreeSet<String>,
    pub pool_group_ids: BTreeSet<AccountGroupId>,
}

impl AccessGroupFields {
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
            return Err(AdminError::invalid("接入分组名称、备注或限额不合法"));
        }
        AccessGroupPolicy::validate_permissions(
            self.limits,
            &self.allowed_models,
            &self.pool_group_ids,
        )
        .map_err(|_| AdminError::invalid("模型白名单或号池选择不合法"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupRecord {
    pub id: AccessGroupId,
    pub fields: AccessGroupFields,
    pub key_count: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupRef {
    pub id: AccessGroupId,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: Option<String>,
}

impl AccessGroupListQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self
                .search
                .as_deref()
                .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("接入分组查询条件不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessGroupPage {
    pub items: Vec<AccessGroupRecord>,
    pub total: u64,
    pub config_revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessGroupChange {
    Create {
        id: AccessGroupId,
        fields: AccessGroupFields,
    },
    Update {
        id: AccessGroupId,
        fields: AccessGroupFields,
    },
    Delete {
        id: AccessGroupId,
    },
    AssignKey {
        key_id: ClientApiKeyId,
        access_group_id: Option<AccessGroupId>,
    },
}

impl AccessGroupChange {
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
pub struct AccessGroupMutation {
    pub id: String,
    pub config_revision: Revision,
}
