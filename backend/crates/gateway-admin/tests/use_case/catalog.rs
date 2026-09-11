use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    sync::Arc,
    time::Duration,
};

use gateway_admin::model::{
    AdminErrorKind, PageSize,
    catalog::{CatalogSourceKind, ModelCatalogQuery},
};
use gateway_core::{
    account::{AccountSelectionPolicy, RotationStrategy},
    catalog::{CatalogModel, CatalogModelKey, ModelCatalogReader, ModelCatalogSnapshot},
    identity::{ChannelId, ProviderKind, SourceId},
    operation::OperationKind,
    routing::{
        ConfigRevision, ModelCapabilities, ModelPresentation, RuntimeSnapshot, UpstreamModelId,
        source::SourceSnapshot,
    },
    runtime::{RuntimeSnapshotHandle, RuntimeSnapshotUnavailable},
};

struct CatalogReader;
impl ModelCatalogReader for CatalogReader {
    fn read(&self) -> Result<ModelCatalogSnapshot, RuntimeSnapshotUnavailable> {
        let provider = ProviderKind::new("openai_api").expect("provider");
        Ok(ModelCatalogSnapshot {
            config_revision: ConfigRevision::new(9007199254740993).expect("revision"),
            provider_generations: BTreeMap::new(),
            items: ["chan_a", "chan_b"]
                .into_iter()
                .enumerate()
                .map(|(index, id)| {
                    let source = SourceId::Channel(ChannelId::new(id).expect("channel"));
                    CatalogModel {
                        key: CatalogModelKey {
                            provider: provider.clone(),
                            upstream_model: UpstreamModelId::new("shared").expect("model"),
                            source: Some(source.clone()),
                        },
                        public_names: vec!["team-alias".to_owned()],
                        source: Some(
                            SourceSnapshot::new(source, Some(format!("Team {index}")))
                                .expect("source"),
                        ),
                        source_controls: None,
                        configuration_ready: index == 0,
                        has_account_source: false,
                        connection_revision: None,
                        capabilities: ModelCapabilities::new(
                            BTreeSet::from([OperationKind::Generate]),
                            None,
                        ),
                        presentation: Some(ModelPresentation::new(
                            Some("Shared model".to_owned()),
                            None,
                        )),
                    }
                })
                .collect(),
        })
    }
}

fn query() -> ModelCatalogQuery {
    ModelCatalogQuery {
        page: 1,
        page_size: PageSize::new(1).expect("size"),
        search: None,
        provider: None,
        source_kind: None,
        configuration_ready: None,
    }
}

#[tokio::test]
async fn catalog_filters_paginate_after_matching_without_merging_sources() {
    let services = super::AdminHarness::new()
        .model_catalog(Arc::new(CatalogReader))
        .build()
        .await;
    let first = services.model_catalog().list(query()).await.expect("page");
    assert_eq!(first.total, 2);
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.config_revision.get(), 9007199254740993);
    let second = services
        .model_catalog()
        .list(ModelCatalogQuery { page: 2, ..query() })
        .await
        .expect("second");
    assert_ne!(first.items[0].key, second.items[0].key);
    for search in ["TEAM 1", "chan_b", "shared", "team-alias"] {
        let result = services
            .model_catalog()
            .list(ModelCatalogQuery {
                search: Some(search.to_owned()),
                configuration_ready: Some(false),
                source_kind: Some(CatalogSourceKind::Channel),
                ..query()
            })
            .await
            .expect("filtered");
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].key, second.items[0].key);
    }
    let absent = services
        .model_catalog()
        .list(ModelCatalogQuery {
            provider: Some(ProviderKind::new("xai").expect("provider")),
            ..query()
        })
        .await
        .expect("no matches");
    assert!(absent.items.is_empty());
    assert_eq!(
        absent.providers.len(),
        1,
        "filter options describe the full snapshot"
    );
    assert!(
        services
            .model_catalog()
            .list(ModelCatalogQuery {
                page: u32::MAX,
                ..query()
            })
            .await
            .expect("out of range")
            .items
            .is_empty()
    );
    assert_eq!(
        services
            .model_catalog()
            .list(ModelCatalogQuery { page: 0, ..query() })
            .await
            .expect_err("invalid")
            .kind(),
        AdminErrorKind::Invalid
    );
}

#[tokio::test]
async fn catalog_distinguishes_an_empty_snapshot_from_runtime_unavailability() {
    let handle = RuntimeSnapshotHandle::default();
    let services = super::AdminHarness::new()
        .model_catalog(Arc::new(handle.clone()))
        .build()
        .await;
    assert_eq!(
        services
            .model_catalog()
            .list(query())
            .await
            .expect_err("unavailable")
            .kind(),
        AdminErrorKind::Unavailable
    );
    handle.publish(
        RuntimeSnapshot::new(
            ConfigRevision::new(1).expect("revision"),
            AccountSelectionPolicy::new(
                RotationStrategy::Smart,
                NonZeroU32::new(1).expect("concurrency"),
                Duration::ZERO,
            ),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("empty snapshot"),
    );
    assert_eq!(
        services
            .model_catalog()
            .list(query())
            .await
            .expect("empty catalog")
            .total,
        0
    );
}
