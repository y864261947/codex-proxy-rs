use std::collections::BTreeMap;

use gateway_core::{
    catalog::CatalogModel,
    identity::{ProviderKind, SourceId},
    routing::ConfigRevision,
};

use super::{AdminError, PageSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSourceKind {
    Channel,
    AccountPool,
    Unpooled,
    ProviderCatalog,
}

impl CatalogSourceKind {
    #[must_use]
    pub fn of(model: &CatalogModel) -> Self {
        match &model.key.source {
            Some(SourceId::Channel(_)) => Self::Channel,
            Some(SourceId::AccountPool(_)) => Self::AccountPool,
            None if model.has_account_source => Self::Unpooled,
            None => Self::ProviderCatalog,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelCatalogQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: Option<String>,
    pub provider: Option<ProviderKind>,
    pub source_kind: Option<CatalogSourceKind>,
    pub configuration_ready: Option<bool>,
}

impl ModelCatalogQuery {
    pub fn validate(&self) -> Result<(), AdminError> {
        if self.page == 0
            || self
                .search
                .as_ref()
                .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(AdminError::invalid("模型目录查询条件不合法"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ModelCatalogPage {
    pub items: Vec<CatalogModel>,
    pub total: u64,
    pub config_revision: ConfigRevision,
    pub provider_generations: BTreeMap<ProviderKind, u64>,
    pub providers: Vec<ProviderKind>,
}
