use gateway_core::{
    account::scope::AccountGroupId,
    identity::{ChannelId, QuotaScopeId},
    policy::RateLimits,
    routing::source::{AllowedSources, SourceId, SourcePolicy, SourcePreference},
};

fn channel_policy(id: &str, enabled: bool) -> SourcePolicy {
    SourcePolicy::new(
        SourceId::Channel(ChannelId::new(id).expect("channel")),
        enabled,
        SourcePreference::default(),
        RateLimits::unlimited(),
        None,
    )
    .expect("policy")
}

fn channel_access(ids: &[&str]) -> AllowedSources {
    AllowedSources::new(
        ids.iter()
            .map(|id| SourceId::Channel(ChannelId::new(*id).expect("channel"))),
    )
}

#[test]
fn source_name_snapshot_and_explicit_no_account_history_survive_later_configuration_changes() {
    use gateway_core::account::scope::{
        AccountRoutingScopeKind, ClientRoutingScope, FrozenAccountScope,
    };
    let first = channel_policy("chan_first", true)
        .with_name("Original channel".to_owned())
        .expect("name");
    let frozen = first.snapshot();
    let renamed = first.with_name("Renamed".to_owned()).expect("rename");
    assert_eq!(frozen.name(), Some("Original channel"));
    assert_eq!(renamed.snapshot().name(), Some("Renamed"));
    assert!(renamed.with_name("bad\nname".to_owned()).is_err());
    let empty = FrozenAccountScope::new(
        super::account_directory(),
        ClientRoutingScope::no_accounts(),
    );
    assert_eq!(
        empty.routing_snapshot().kind(),
        AccountRoutingScopeKind::None
    );
    assert!(empty.routing_snapshot().groups_snapshot().is_empty());
    assert!(
        !empty.allows(
            &gateway_core::account::ProviderAccountId::new("acct_openai").expect("account")
        )
    );
}

#[test]
fn same_named_channel_models_keep_capabilities_and_visibility_separate() {
    use gateway_core::routing::{
        ConfigRevision, ModelCapabilities, ProviderKind, PublicModelId, RoutingContext,
        RuntimeSnapshot,
    };
    use std::collections::{BTreeMap, BTreeSet};
    let first = ChannelId::new("chan_first").expect("channel");
    let second = ChannelId::new("chan_second").expect("channel");
    let third = ChannelId::new("chan_disabled").expect("channel");
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        vec![
            super::model("openai", "shared", super::capabilities()).with_channel(first),
            super::model(
                "openai",
                "shared",
                ModelCapabilities::new(BTreeSet::new(), None),
            )
            .with_channel(second.clone()),
            super::model("openai", "second-only", super::capabilities()).with_channel(second),
            super::model("openai", "disabled-only", super::capabilities()).with_channel(third),
        ],
        Vec::new(),
    )
    .expect("same upstream model in different channels is valid")
    .with_account_directory(super::account_directory())
    .with_model_mappings(BTreeMap::from([
        ("public-first".to_owned(), "shared".to_owned()),
        ("secret-alias".to_owned(), "second-only".to_owned()),
        ("absent-alias".to_owned(), "absent".to_owned()),
    ]))
    .with_source_policies(vec![
        channel_policy("chan_first", true),
        channel_policy("chan_second", true),
        channel_policy("chan_disabled", false),
    ])
    .expect("policies");
    let first_access = channel_access(&["chan_first"]);
    assert_eq!(
        snapshot
            .public_models_for_channels(&first_access)
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["public-first", "shared"]
    );
    assert!(
        snapshot
            .public_models_for_channels(&AllowedSources::default())
            .is_empty()
    );
    assert!(
        snapshot
            .public_models_for_channels(&channel_access(&["chan_disabled", "chan_absent"]))
            .is_empty()
    );
    let shared = PublicModelId::new("shared").expect("model");
    let plan = snapshot
        .plan_channels(
            &shared,
            &super::operation(),
            snapshot.all_account_scope(),
            &RoutingContext::default(),
            &first_access,
        )
        .expect("first supports generation");
    assert_eq!(plan.candidates().len(), 1);
    assert_eq!(
        plan.candidates()[0].source(),
        Some(&SourceId::Channel(
            ChannelId::new("chan_first").expect("channel")
        ))
    );
    assert!(
        snapshot
            .plan_channels(
                &shared,
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default(),
                &channel_access(&["chan_second"])
            )
            .is_err()
    );
    assert!(
        snapshot
            .plan_channels(
                &PublicModelId::new("secret-alias").expect("model"),
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default(),
                &first_access
            )
            .is_err()
    );
    assert!(
        snapshot
            .plan_channels(
                &shared,
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext {
                    blocked_providers: BTreeSet::from([
                        ProviderKind::new("openai").expect("provider")
                    ]),
                    ..RoutingContext::default()
                },
                &first_access
            )
            .is_err()
    );
    // 相同适配器的旧账号路径不能从渠道目录获得能力或隐式授权。
    assert!(
        snapshot
            .plan(
                &shared,
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default()
            )
            .is_err()
    );
    assert!(
        snapshot
            .public_models_for_scope(&snapshot.all_account_scope())
            .iter()
            .all(|model| model.as_str() != "second-only")
    );
}

#[test]
fn missing_source_policy_denies_a_discovered_channel_and_duplicate_catalog_rows_are_rejected() {
    use gateway_core::routing::{
        ConfigRevision, ProviderKind, PublicModelId, RoutingContext, RuntimeSnapshot,
    };
    let channel = ChannelId::new("chan_first").expect("channel");
    let model =
        super::model("openai", "shared", super::capabilities()).with_channel(channel.clone());
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        vec![model.clone()],
        Vec::new(),
    )
    .expect("snapshot");
    let allowed = channel_access(&["chan_first"]);
    assert!(snapshot.public_models_for_channels(&allowed).is_empty());
    assert!(
        snapshot
            .plan_channels(
                &PublicModelId::new("shared").expect("model"),
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default(),
                &allowed
            )
            .is_err()
    );
    assert!(
        RuntimeSnapshot::new(
            ConfigRevision::new(1).expect("revision"),
            super::scheduling(),
            vec![ProviderKind::new("openai").expect("provider")],
            vec![model.clone(), model.clone()],
            Vec::new()
        )
        .is_err()
    );
    let other = super::model("xai", "different", super::capabilities()).with_channel(channel);
    assert!(
        RuntimeSnapshot::new(
            ConfigRevision::new(1).expect("revision"),
            super::scheduling(),
            vec![
                ProviderKind::new("openai").expect("provider"),
                ProviderKind::new("xai").expect("provider")
            ],
            vec![model, other],
            Vec::new()
        )
        .is_err()
    );
    assert!(
        snapshot
            .with_source_policies(vec![
                channel_policy("chan_first", true),
                channel_policy("chan_first", false)
            ])
            .is_err()
    );
}

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
