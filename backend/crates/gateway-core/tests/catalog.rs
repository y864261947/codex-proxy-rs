use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use gateway_core::{
    account::ProviderAccountId,
    catalog::ModelCatalogReader,
    channel::{ChannelBinding, ChannelRevision},
    identity::{AccountGroupId, ChannelId, QuotaScopeId, SourceId},
    operation::{Feature, OperationKind},
    policy::{RateLimits, SourcePreference},
    routing::{
        ConfigRevision, ModelPresentation, ProviderKind, RuntimeAccount, RuntimeAccountDirectory,
        RuntimeSnapshot, SupportLevel,
        source::{QuotaScopePolicy, SourcePolicy},
    },
    runtime::RuntimeSnapshotHandle,
};

#[test]
fn catalog_preserves_source_identity_aliases_capability_evidence_and_configuration_boundaries() {
    let pool = AccountGroupId::new("grp_00000000000000000000000000000001").expect("pool");
    let channel = ChannelId::new("chan_catalog").expect("channel");
    let quota = QuotaScopeId::new("quota_catalog").expect("quota");
    let provider = ProviderKind::new("openai").expect("provider");
    let api_provider = ProviderKind::new("openai_api").expect("API provider");
    let pool_source = SourceId::AccountPool(pool.clone());
    let channel_source = SourceId::Channel(channel.clone());
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(9).expect("revision"),
        scheduling(),
        vec![provider.clone(), api_provider],
        vec![
            model(
                "openai",
                "shared",
                capabilities().with_feature(Feature::Tools, SupportLevel::Native),
            )
            .with_presentation(
                ModelPresentation::new(Some("Shared model".to_owned()), None)
                    .with_context_window_tokens(Some(128000)),
            ),
            model("openai", "shadow", capabilities()),
            model(
                "openai_api",
                "shared",
                capabilities().with_upstream_feature_validation(),
            )
            .with_channel(ChannelBinding::new(
                channel,
                ChannelRevision::new(5).expect("version"),
            )),
        ],
        Vec::new(),
    )
    .expect("snapshot")
    .with_model_mappings(BTreeMap::from([
        ("alias".to_owned(), "shared".to_owned()),
        ("chain".to_owned(), "alias".to_owned()),
        ("shadow".to_owned(), "shared".to_owned()),
    ]))
    .with_account_directory(Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
        (
            ProviderAccountId::new("acct_pooled").expect("account"),
            RuntimeAccount::new(provider.clone(), BTreeSet::from([pool])),
        ),
        (
            ProviderAccountId::new("acct_unpooled").expect("account"),
            RuntimeAccount::new(provider, BTreeSet::new()),
        ),
    ]))))
    .with_source_policies(vec![
        SourcePolicy::new(
            pool_source.clone(),
            true,
            SourcePreference::default(),
            RateLimits::unlimited(),
            Some(quota.clone()),
        )
        .expect("pool"),
        SourcePolicy::new(
            channel_source.clone(),
            true,
            SourcePreference::default(),
            RateLimits::unlimited(),
            None,
        )
        .expect("channel"),
    ])
    .expect("policies")
    .with_quota_policies(vec![
        QuotaScopePolicy::new(quota, false, RateLimits::unlimited()).expect("quota"),
    ])
    .expect("quotas");
    let handle = RuntimeSnapshotHandle::new(snapshot);
    let catalog = handle.read().expect("catalog");
    let pool = catalog
        .items
        .iter()
        .find(|item| {
            item.key.source.as_ref() == Some(&pool_source)
                && item.key.upstream_model.as_str() == "shared"
        })
        .expect("pool row");
    let channel = catalog
        .items
        .iter()
        .find(|item| item.key.source.as_ref() == Some(&channel_source))
        .expect("channel row");
    assert!(
        !pool.configuration_ready,
        "disabled shared quota is not ready"
    );
    assert!(channel.configuration_ready);
    assert_ne!(pool.key, channel.key);
    assert_eq!(pool.public_names, ["alias", "chain", "shadow", "shared"]);
    assert_eq!(channel.public_names, pool.public_names);
    assert!(
        catalog
            .items
            .iter()
            .filter(|item| item.key.upstream_model.as_str() == "shadow")
            .all(|item| item.public_names.is_empty())
    );
    assert_eq!(channel.connection_revision.expect("version").get(), 5);
    assert_eq!(
        pool.capabilities.features().get(&Feature::Tools),
        Some(&SupportLevel::Native)
    );
    assert!(
        !channel
            .capabilities
            .features()
            .contains_key(&Feature::Tools),
        "upstream validation is not proof of tool support"
    );
    assert!(channel.capabilities.upstream_validates_features());
    assert!(
        !channel
            .capabilities
            .operations()
            .contains(&OperationKind::GenerateImage)
    );
    assert!(catalog.items.iter().any(|item| item.key.source.is_none()
        && item.has_account_source
        && item.configuration_ready));
    handle.suspend();
    assert!(
        handle.read().is_err(),
        "unavailable snapshots must not become an empty catalog"
    );
    assert_eq!(catalog.config_revision.get(), 9);
    assert!(
        channel.configuration_ready,
        "previous projections remain immutable"
    );
}

#[test]
fn catalog_without_accounts_remains_visible_but_has_no_callable_source() {
    let catalog = snapshot()
        .with_account_directory(Arc::new(RuntimeAccountDirectory::default()))
        .model_catalog();
    assert!(!catalog.items.is_empty());
    for item in catalog.items {
        assert!(item.key.source.is_none());
        assert!(!item.configuration_ready);
        assert!(!item.has_account_source);
    }
}

fn scheduling() -> gateway_core::account::AccountSelectionPolicy {
    gateway_core::account::AccountSelectionPolicy::new(
        gateway_core::account::RotationStrategy::Smart,
        std::num::NonZeroU32::new(3).expect("concurrency"),
        std::time::Duration::ZERO,
    )
}
fn capabilities() -> gateway_core::routing::ModelCapabilities {
    gateway_core::routing::ModelCapabilities::new(
        BTreeSet::from([OperationKind::Generate]),
        Some(16000),
    )
}
fn model(
    provider: &str,
    name: &str,
    capabilities: gateway_core::routing::ModelCapabilities,
) -> gateway_core::routing::ProviderModel {
    gateway_core::routing::ProviderModel::new(
        ProviderKind::new(provider).expect("provider"),
        gateway_core::routing::UpstreamModelId::new(name).expect("model"),
        capabilities,
    )
}
fn snapshot() -> RuntimeSnapshot {
    RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        vec![model("openai", "shared", capabilities())],
        Vec::new(),
    )
    .expect("snapshot")
}
