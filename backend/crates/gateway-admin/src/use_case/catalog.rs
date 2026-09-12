use async_trait::async_trait;
use gateway_core::catalog::SharedModelCatalogReader;

use crate::model::{
    AdminError,
    catalog::{CatalogSourceKind, ModelCatalogPage, ModelCatalogQuery},
};

#[async_trait]
pub trait ModelCatalogService: Send + Sync {
    async fn list(&self, query: ModelCatalogQuery) -> Result<ModelCatalogPage, AdminError>;
}

pub(crate) struct DefaultModelCatalogService {
    reader: SharedModelCatalogReader,
}

impl DefaultModelCatalogService {
    pub(crate) fn new(reader: SharedModelCatalogReader) -> Self {
        Self { reader }
    }
}

#[async_trait]
impl ModelCatalogService for DefaultModelCatalogService {
    async fn list(&self, query: ModelCatalogQuery) -> Result<ModelCatalogPage, AdminError> {
        query.validate()?;
        let snapshot = self
            .reader
            .read()
            .map_err(|_| AdminError::unavailable("运行模型目录暂不可用"))?;
        let providers = snapshot
            .items
            .iter()
            .map(|item| item.key.provider.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let search = query.search.as_deref().unwrap_or("").to_lowercase();
        let mut matched = snapshot
            .items
            .into_iter()
            .filter(|item| {
                query
                    .provider
                    .as_ref()
                    .is_none_or(|provider| provider == &item.key.provider)
                    && query
                        .source_kind
                        .is_none_or(|kind| kind == CatalogSourceKind::of(item))
                    && query
                        .configuration_ready
                        .is_none_or(|ready| ready == item.configuration_ready)
                    && (search.is_empty()
                        || item
                            .key
                            .upstream_model
                            .as_str()
                            .to_lowercase()
                            .contains(&search)
                        || item
                            .public_names
                            .iter()
                            .any(|name| name.to_lowercase().contains(&search))
                        || item
                            .source
                            .as_ref()
                            .and_then(|source| source.name())
                            .is_some_and(|name| name.to_lowercase().contains(&search))
                        || item.key.source.as_ref().is_some_and(|source| {
                            source.reference().to_lowercase().contains(&search)
                        })
                        || item
                            .presentation
                            .as_ref()
                            .and_then(|presentation| presentation.display_name())
                            .is_some_and(|name| name.to_lowercase().contains(&search)))
            })
            .collect::<Vec<_>>();
        let total = matched.len() as u64;
        let offset = u64::from(query.page - 1) * u64::from(query.page_size.get());
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(matched.len());
        let end = start
            .saturating_add(usize::from(query.page_size.get()))
            .min(matched.len());
        let items = matched.drain(start..end).collect();
        Ok(ModelCatalogPage {
            items,
            total,
            config_revision: snapshot.config_revision,
            provider_generations: snapshot.provider_generations,
            providers,
        })
    }
}
