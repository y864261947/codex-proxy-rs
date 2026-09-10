use gateway_core::{
    account::scope::AccountGroupId,
    identity::{ChannelId, QuotaScopeId},
    policy::RateLimits,
    routing::source::{AllowedSources, SourceId, SourcePolicy, SourcePreference},
};

#[test]
fn source_preferences_do_not_replace_global_capacity_or_shared_quota_identity() {
    let quota = QuotaScopeId::new("quota_project").expect("quota");
    let limits = RateLimits {
        max_concurrency: 20,
        requests_per_minute: 300,
    };
    let channel = SourcePolicy::new(
        SourceId::Channel(ChannelId::new("chan_first").expect("channel")),
        true,
        SourcePreference::new(2, 3).expect("preference"),
        limits,
        Some(quota.clone()),
    )
    .expect("source");
    let override_value = SourcePreference::new(1, 10).expect("override");
    assert_eq!(channel.effective_preference(None).priority(), 2);
    assert_eq!(
        channel
            .effective_preference(Some(override_value))
            .priority(),
        1
    );
    assert_eq!(
        channel.effective_preference(Some(override_value)).weight(),
        10
    );
    assert_eq!(channel.limits(), limits);
    assert_eq!(channel.quota_scope_id(), Some(&quota));
    assert!(SourcePreference::new(0, 1).is_err());
    assert!(SourcePreference::new(1, 0).is_err());
    assert!(
        SourcePolicy::new(
            channel.id().clone(),
            true,
            SourcePreference::default(),
            RateLimits {
                max_concurrency: u64::MAX,
                requests_per_minute: 0
            },
            None
        )
        .is_err()
    );
}

#[test]
fn source_allowlist_is_explicit_empty_denies_and_account_pool_ids_do_not_alias_channels() {
    let pool = SourceId::AccountPool(
        AccountGroupId::new("grp_00000000000000000000000000000001").expect("pool"),
    );
    let channel = SourceId::Channel(ChannelId::new("chan_one").expect("channel"));
    assert!(!AllowedSources::default().allows(&pool));
    assert!(!AllowedSources::default().allows(&channel));
    let allowed = AllowedSources::new([pool.clone(), pool.clone()]);
    assert_eq!(allowed.iter().count(), 1);
    assert!(allowed.allows(&pool));
    assert!(!allowed.allows(&channel));
    assert_ne!(pool.to_string(), channel.to_string());
}

#[test]
fn call_metadata_cannot_substitute_another_channel_or_an_account_for_the_selected_source() {
    use gateway_core::{
        account::ProviderAccountId,
        engine::provider::ProviderCallMetadata,
        routing::{PublicModelId, RoutingContext},
        upstream::UpstreamTransport,
    };

    let snapshot = super::snapshot();
    let plan = snapshot
        .plan(
            &PublicModelId::new("gpt-5.5").expect("model"),
            &super::operation(),
            snapshot.all_account_scope(),
            &RoutingContext::default(),
        )
        .expect("plan");
    let legacy = &plan.candidates()[0];
    let first = ChannelId::new("chan_first").expect("channel");
    let second = ChannelId::new("chan_second").expect("channel");
    let selected = legacy.clone().with_source(SourceId::Channel(first.clone()));
    let metadata = |channel| {
        ProviderCallMetadata::for_channel(
            legacy.provider().clone(),
            legacy.upstream_model().cloned(),
            channel,
            UpstreamTransport::new("http_sse").expect("transport"),
        )
    };
    assert!(metadata(first.clone()).confirms(&selected));
    assert!(!metadata(second).confirms(&selected));
    assert!(!metadata(first).confirms(legacy));
    let account = ProviderCallMetadata::new(
        legacy.provider().clone(),
        legacy.upstream_model().expect("model").clone(),
        ProviderAccountId::new("acct_openai").expect("account"),
        UpstreamTransport::new("http_sse").expect("transport"),
    );
    assert!(account.confirms(legacy));
    assert!(!account.confirms(&selected));
}

#[test]
fn selected_pool_requires_the_actual_account_to_belong_to_that_pool_and_provider() {
    use gateway_core::{
        account::{
            ProviderAccountId,
            scope::{RuntimeAccount, RuntimeAccountDirectory},
        },
        engine::provider::ProviderCallMetadata,
        identity::ProviderKind,
        routing::{PublicModelId, RoutingContext},
        upstream::UpstreamTransport,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    let pool = AccountGroupId::new("grp_11111111111111111111111111111111").expect("pool");
    let provider = ProviderKind::new("openai").expect("provider");
    let member = ProviderAccountId::new("acct_member").expect("account");
    let outsider = ProviderAccountId::new("acct_outsider").expect("account");
    let wrong_provider = ProviderAccountId::new("acct_wrong_provider").expect("account");
    let snapshot = super::snapshot().with_account_directory(Arc::new(
        RuntimeAccountDirectory::new(BTreeMap::from([
            (
                member.clone(),
                RuntimeAccount::new(provider.clone(), BTreeSet::from([pool.clone()])),
            ),
            (
                outsider.clone(),
                RuntimeAccount::new(provider, BTreeSet::new()),
            ),
            (
                wrong_provider.clone(),
                RuntimeAccount::new(
                    ProviderKind::new("xai").expect("provider"),
                    BTreeSet::from([pool.clone()]),
                ),
            ),
        ])),
    ));
    let plan = snapshot
        .plan(
            &PublicModelId::new("gpt-5.5").expect("model"),
            &super::operation(),
            snapshot.all_account_scope(),
            &RoutingContext::default(),
        )
        .expect("plan");
    let candidate = plan.candidates()[0]
        .clone()
        .with_source(SourceId::AccountPool(pool));
    let metadata = |account| {
        ProviderCallMetadata::new(
            candidate.provider().clone(),
            candidate.upstream_model().expect("model").clone(),
            account,
            UpstreamTransport::new("http_sse").expect("transport"),
        )
    };
    assert!(metadata(member).confirms(&candidate));
    assert!(!metadata(outsider).confirms(&candidate));
    assert!(!metadata(wrong_provider).confirms(&candidate));
    assert!(
        !metadata(ProviderAccountId::new("acct_absent").expect("account")).confirms(&candidate)
    );
}
