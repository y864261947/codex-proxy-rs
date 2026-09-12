use std::{collections::BTreeMap, sync::Arc};

use crate::{
    channel::ChannelRevision,
    identity::ProviderKind,
    routing::{
        ConfigRevision, ModelCapabilities, ModelPresentation, UpstreamModelId,
        source::{SourceControls, SourceId, SourceSnapshot},
    },
    runtime::{RuntimeSnapshotHandle, RuntimeSnapshotUnavailable},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CatalogModelKey {
    pub provider: ProviderKind,
    pub upstream_model: UpstreamModelId,
    pub source: Option<SourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub key: CatalogModelKey,
    pub public_names: Vec<String>,
    pub source: Option<SourceSnapshot>,
    pub source_controls: Option<SourceControls>,
    pub configuration_ready: bool,
    pub has_account_source: bool,
    pub connection_revision: Option<ChannelRevision>,
    pub capabilities: ModelCapabilities,
    pub presentation: Option<ModelPresentation>,
}

#[derive(Debug, Clone)]
pub struct ModelCatalogSnapshot {
    pub config_revision: ConfigRevision,
    pub provider_generations: BTreeMap<ProviderKind, u64>,
    pub items: Vec<CatalogModel>,
}

pub trait ModelCatalogReader: Send + Sync {
    fn read(&self) -> Result<ModelCatalogSnapshot, RuntimeSnapshotUnavailable>;
}

impl ModelCatalogReader for RuntimeSnapshotHandle {
    fn read(&self) -> Result<ModelCatalogSnapshot, RuntimeSnapshotUnavailable> {
        self.acquire().map(|snapshot| snapshot.model_catalog())
    }
}

pub type SharedModelCatalogReader = Arc<dyn ModelCatalogReader>;
