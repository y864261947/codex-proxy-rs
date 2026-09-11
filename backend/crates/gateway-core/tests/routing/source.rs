use gateway_core::{
    account::scope::AccountGroupId,
    channel::{ChannelBinding, ChannelRevision},
    identity::{ChannelId, QuotaScopeId},
    policy::RateLimits,
    routing::source::{AllowedSources, SourceId, SourcePolicy, SourcePreference},
};

fn binding(id: ChannelId) -> ChannelBinding {
    ChannelBinding::new(id, ChannelRevision::new(1).expect("channel revision"))
}

#[test]
fn explicit_provider_cannot_expand_an_empty_or_other_provider_account_scope() {
    use gateway_core::{
        account::ProviderAccountId,
        routing::{
            ClientRoutingScope, FrozenAccountScope, ProviderKind, PublicModelId, RoutingContext,
            RuntimeAccount, RuntimeAccountDirectory, SourceRoutingTarget,
        },
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    let snapshot = super::snapshot();
    let provider = ProviderKind::new("openai").expect("provider");
    let context = RoutingContext {
        required_provider: Some(provider.clone()),
        ..RoutingContext::default()
    };
    let model = PublicModelId::new("gpt-5.5").expect("model");
    let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
        ProviderAccountId::new("acct_other").expect("account"),
        RuntimeAccount::new(
            ProviderKind::new("xai").expect("other provider"),
            BTreeSet::new(),
        ),
    )])));
    for permissions in [
        ClientRoutingScope::no_accounts(),
        ClientRoutingScope::all_accounts(),
    ] {
        let scope = Arc::new(FrozenAccountScope::new(directory.clone(), permissions));
        assert!(
            snapshot
                .plan(&model, &super::operation(), scope.clone(), &context)
                .is_err()
        );
        assert!(
            snapshot
                .plan_account_sources(
                    SourceRoutingTarget::Model(&model),
                    &super::operation(),
                    scope.clone(),
                    &context,
                    0
                )
                .is_err()
        );
        assert!(
            snapshot
                .plan_account_sources(
                    SourceRoutingTarget::ProviderEndpoint(&provider),
                    &super::operation(),
                    scope,
                    &context,
                    0
                )
                .is_err()
        );
    }
}

#[test]
fn legacy_account_routes_cannot_bypass_pool_state_or_grant_channels_through_unpooled_fallback() {
    use gateway_core::{
        account::ProviderAccountId,
        routing::{
            ConfigRevision, ProviderKind, PublicModelId, RoutingContext, RoutingGroupSnapshot,
            RuntimeAccount, RuntimeAccountDirectory, RuntimeSnapshot, SourceRoutingTarget,
        },
    };
    use std::{collections::BTreeSet, sync::Arc};
    let account = |id: &str| ProviderAccountId::new(id).expect("account");
    let provider = ProviderKind::new("openai").expect("provider");
    let a = AccountGroupId::new("grp_00000000000000000000000000000001").expect("A");
    let b = AccountGroupId::new("grp_00000000000000000000000000000002").expect("B");
    let source_a = SourceId::AccountPool(a.clone());
    let directory = Arc::new(RuntimeAccountDirectory::new(
        [
            ("acct_a", BTreeSet::from([a.clone()])),
            ("acct_b", BTreeSet::from([b.clone()])),
            ("acct_shared", BTreeSet::from([a, b.clone()])),
            ("acct_unpooled", BTreeSet::new()),
        ]
        .into_iter()
        .map(|(id, groups)| (account(id), RuntimeAccount::new(provider.clone(), groups)))
        .collect(),
    ));
    let quota = QuotaScopeId::new("quota_legacy").expect("quota");
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![provider.clone()],
        vec![
            super::model("openai", "shared", super::capabilities()),
            super::model("openai", "shared", super::capabilities())
                .with_channel(binding(ChannelId::new("chan_hidden").expect("channel"))),
        ],
        Vec::new(),
    )
    .expect("snapshot")
    .with_account_directory(directory)
    .with_source_policies(vec![
        SourcePolicy::new(
            source_a.clone(),
            true,
            SourcePreference::default(),
            RateLimits {
                max_concurrency: 3,
                requests_per_minute: 40,
            },
            Some(quota.clone()),
        )
        .expect("A"),
        SourcePolicy::new(
            SourceId::AccountPool(b.clone()),
            false,
            SourcePreference::default(),
            RateLimits::unlimited(),
            None,
        )
        .expect("B"),
        channel_policy("chan_hidden", true),
    ])
    .expect("policies")
    .with_quota_policies(vec![
        gateway_core::routing::source::QuotaScopePolicy::new(
            quota,
            true,
            RateLimits {
                max_concurrency: 2,
                requests_per_minute: 30,
            },
        )
        .expect("quota"),
    ])
    .expect("quotas");
    let model = PublicModelId::new("shared").expect("model");
    for target in [
        SourceRoutingTarget::Model(&model),
        SourceRoutingTarget::ProviderEndpoint(&provider),
    ] {
        let all = snapshot.all_account_scope();
        let plan = snapshot
            .plan_account_sources(
                target,
                &super::operation(),
                all.clone(),
                &RoutingContext::default(),
                0,
            )
            .expect("plan");
        assert_eq!(plan.candidates().len(), 2);
        let pooled = &plan.candidates()[0];
        assert_eq!(pooled.source(), Some(&source_a));
        assert_eq!(pooled.source_controls().limits().max_concurrency, 3);
        assert_eq!(
            pooled
                .shared_quota()
                .expect("shared quota")
                .limits()
                .max_concurrency,
            2
        );
        assert!(pooled.account_scope().allows(&account("acct_a")));
        assert!(pooled.account_scope().allows(&account("acct_shared")));
        let unpooled = &plan.candidates()[1];
        assert!(unpooled.source().is_none());
        assert!(unpooled.account_scope().allows(&account("acct_unpooled")));
        for id in ["acct_a", "acct_b", "acct_shared"] {
            assert!(!unpooled.account_scope().allows(&account(id)));
        }
        assert_eq!(
            plan.account_scope().routing_snapshot(),
            all.routing_snapshot()
        );
        for context in [
            RoutingContext {
                blocked_sources: BTreeSet::from([source_a.clone()]),
                ..RoutingContext::default()
            },
            RoutingContext {
                blocked_providers: BTreeSet::from([provider.clone()]),
                ..RoutingContext::default()
            },
        ] {
            let filtered = snapshot
                .plan_account_sources(target, &super::operation(), all.clone(), &context, 0)
                .expect("independent scope remains");
            assert_eq!(filtered.candidates().len(), 1);
            assert_eq!(
                filtered.candidates()[0].source().is_none(),
                !context.blocked_sources.is_empty()
            );
        }
        let fixed = Arc::new(all.only_account(&account("acct_a")));
        let diagnostic_context = RoutingContext {
            required_provider: Some(provider.clone()),
            ..RoutingContext::default()
        };
        let fixed_plan = snapshot
            .plan_account_sources(target, &super::operation(), fixed, &diagnostic_context, 0)
            .expect("fixed A");
        assert_eq!(fixed_plan.candidates().len(), 1);
        assert!(
            !fixed_plan.candidates()[0]
                .account_scope()
                .allows(&account("acct_shared"))
        );
        for scope in [
            all.only_account(&account("acct_b")),
            all.within_group(RoutingGroupSnapshot::new(b.clone(), "B".to_owned())),
        ] {
            assert!(
                snapshot
                    .plan_account_sources(
                        target,
                        &super::operation(),
                        Arc::new(scope),
                        &diagnostic_context,
                        0
                    )
                    .is_err()
            );
        }
        let missing = snapshot
            .clone()
            .with_source_policies(Vec::new())
            .expect("removed policies");
        let only_free = missing
            .plan_account_sources(
                target,
                &super::operation(),
                all,
                &RoutingContext::default(),
                0,
            )
            .expect("unpooled still available");
        assert_eq!(only_free.candidates().len(), 1);
        assert!(only_free.candidates()[0].source().is_none());
        assert!(
            !only_free.candidates()[0]
                .account_scope()
                .allows(&account("acct_shared"))
        );
    }
}

#[test]
fn one_channel_catalog_cannot_combine_models_from_different_configuration_versions() {
    use gateway_core::routing::{ConfigRevision, ProviderKind, RuntimeSnapshot};
    let id = ChannelId::new("chan_versions").expect("channel");
    let first = super::model("openai", "first-model", super::capabilities())
        .with_channel(binding(id.clone()));
    let second = super::model("openai", "second-model", super::capabilities()).with_channel(
        ChannelBinding::new(id, ChannelRevision::new(2).expect("revision")),
    );
    assert!(
        RuntimeSnapshot::new(
            ConfigRevision::new(1).expect("revision"),
            super::scheduling(),
            vec![ProviderKind::new("openai").expect("provider")],
            vec![first, second],
            Vec::new()
        )
        .is_err()
    );
}

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
fn pool_source_plans_intersect_account_and_provider_permissions() {
    use gateway_core::account::ProviderAccountId;
    use gateway_core::routing::{
        ClientRoutingScope, FrozenAccountScope, ProviderKind, PublicModelId, RoutingContext,
        RoutingGroupSnapshot, RuntimeAccount, RuntimeAccountDirectory, SourceRoutingTarget,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };
    let a = AccountGroupId::new("grp_11111111111111111111111111111111").expect("pool A");
    let b = AccountGroupId::new("grp_22222222222222222222222222222222").expect("pool B");
    let provider = ProviderKind::new("openai").expect("provider");
    let account_a = ProviderAccountId::new("acct_pool_a").expect("account A");
    let account_b = ProviderAccountId::new("acct_pool_b").expect("account B");
    let other_provider = ProviderAccountId::new("acct_other_provider").expect("account");
    let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
        (
            account_a.clone(),
            RuntimeAccount::new(provider.clone(), BTreeSet::from([a.clone()])),
        ),
        (
            account_b.clone(),
            RuntimeAccount::new(provider.clone(), BTreeSet::from([b.clone()])),
        ),
        (
            other_provider.clone(),
            RuntimeAccount::new(
                ProviderKind::new("xai").expect("xai"),
                BTreeSet::from([a.clone()]),
            ),
        ),
    ])));
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::clone(&directory),
        ClientRoutingScope::restricted(
            vec![RoutingGroupSnapshot::new(a.clone(), "Pool A".to_owned())],
            BTreeSet::from([a.clone()]),
            BTreeSet::from([provider.clone()]),
        )
        .expect("restricted"),
    ));
    let snapshot = super::snapshot()
        .with_account_directory(Arc::clone(&directory))
        .with_source_policies(vec![
            SourcePolicy::new(
                SourceId::AccountPool(a.clone()),
                true,
                SourcePreference::default(),
                RateLimits::unlimited(),
                None,
            )
            .expect("A")
            .with_name("Pool A".to_owned())
            .expect("name"),
            SourcePolicy::new(
                SourceId::AccountPool(b.clone()),
                true,
                SourcePreference::default(),
                RateLimits::unlimited(),
                None,
            )
            .expect("B"),
        ])
        .expect("sources");
    // Even an overly broad source set cannot expand the independently frozen account scope.
    let allowed = AllowedSources::new([
        SourceId::AccountPool(a.clone()),
        SourceId::AccountPool(b.clone()),
    ]);
    let model = PublicModelId::new("gpt-5.5").expect("model");
    for target in [
        SourceRoutingTarget::Model(&model),
        SourceRoutingTarget::ProviderEndpoint(&provider),
    ] {
        let plan = snapshot
            .plan_sources(
                target,
                &super::operation(),
                Arc::clone(&scope),
                &RoutingContext::default(),
                &allowed,
                42,
            )
            .expect("pool plan");
        assert_eq!(plan.candidates().len(), 1);
        let candidate = &plan.candidates()[0];
        assert_eq!(candidate.source(), Some(&SourceId::AccountPool(a.clone())));
        assert_eq!(
            candidate.source_snapshot().and_then(|source| source.name()),
            Some("Pool A")
        );
        assert!(candidate.account_scope().allows(&account_a));
        assert!(!candidate.account_scope().allows(&account_b));
        assert!(!candidate.account_scope().allows(&other_provider));
        assert_eq!(
            candidate.account_scope().provider_kinds(),
            &BTreeSet::from([provider.clone()])
        );
    }
    for denied in [
        Arc::new(FrozenAccountScope::new(
            Arc::clone(&directory),
            ClientRoutingScope::no_accounts(),
        )),
        Arc::new(scope.within_group(RoutingGroupSnapshot::new(b, "Unapproved".to_owned()))),
    ] {
        assert!(
            snapshot
                .plan_sources(
                    SourceRoutingTarget::Model(&model),
                    &super::operation(),
                    denied,
                    &RoutingContext::default(),
                    &allowed,
                    0
                )
                .is_err()
        );
    }
    assert!(
        snapshot
            .plan_sources(
                SourceRoutingTarget::Model(&model),
                &super::operation(),
                scope,
                &RoutingContext::default(),
                &AllowedSources::default(),
                0
            )
            .is_err()
    );
}

#[test]
fn source_priority_precedes_weight_and_order_is_frozen_without_duplicates() {
    use gateway_core::routing::{
        ConfigRevision, ProviderKind, PublicModelId, RoutingContext, RuntimeSnapshot,
        SourceRoutingTarget,
    };
    let ids = ["chan_one", "chan_three", "chan_fallback"];
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        ids.iter()
            .map(|id| {
                super::model("openai", "gpt-5.5", super::capabilities())
                    .with_channel(binding(ChannelId::new(*id).expect("channel")))
            })
            .collect(),
        Vec::new(),
    )
    .expect("snapshot")
    .with_source_policies(
        ids.iter()
            .zip([(1, 1), (1, 3), (2, 1000)])
            .map(|(id, (priority, weight))| {
                SourcePolicy::new(
                    SourceId::Channel(ChannelId::new(*id).expect("channel")),
                    true,
                    SourcePreference::new(priority, weight).expect("preference"),
                    RateLimits::unlimited(),
                    None,
                )
                .expect("policy")
            })
            .collect(),
    )
    .expect("policies");
    let access = channel_access(&ids);
    let model = PublicModelId::new("gpt-5.5").expect("model");
    let order = |seed| {
        snapshot
            .plan_sources(
                SourceRoutingTarget::Model(&model),
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default(),
                &access,
                seed,
            )
            .expect("plan")
            .candidates()
            .iter()
            .map(|candidate| candidate.source().expect("source").reference().to_owned())
            .collect::<Vec<_>>()
    };
    let mut first_count = 0;
    for seed in 0..1_000 {
        let chosen = order(seed);
        assert_eq!(chosen.len(), 3);
        assert_eq!(chosen[2], "chan_fallback");
        assert_ne!(chosen[0], chosen[1]);
        assert_eq!(
            chosen,
            order(seed),
            "same request seed freezes the attempt order"
        );
        first_count += usize::from(chosen[0] == "chan_one");
    }
    assert!(
        (200..=300).contains(&first_count),
        "1:3 weighting was not observed: {first_count}"
    );
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
            super::model("openai", "shared", super::capabilities()).with_channel(binding(first)),
            super::model(
                "openai",
                "shared",
                ModelCapabilities::new(BTreeSet::new(), None),
            )
            .with_channel(binding(second.clone())),
            super::model("openai", "second-only", super::capabilities())
                .with_channel(binding(second)),
            super::model("openai", "disabled-only", super::capabilities())
                .with_channel(binding(third)),
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
    let model = super::model("openai", "shared", super::capabilities())
        .with_channel(binding(channel.clone()));
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
    let other =
        super::model("xai", "different", super::capabilities()).with_channel(binding(channel));
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
    let selected = legacy.clone().with_channel(binding(first.clone()));
    let metadata = |channel| {
        ProviderCallMetadata::for_channel(
            legacy.provider().clone(),
            legacy.upstream_model().cloned(),
            binding(channel),
            UpstreamTransport::new("http_sse").expect("transport"),
        )
    };
    assert!(metadata(first.clone()).confirms(&selected));
    let rotated = selected.clone().with_channel(ChannelBinding::new(
        first.clone(),
        ChannelRevision::new(2).expect("new revision"),
    ));
    assert!(!metadata(first.clone()).confirms(&rotated));
    let unversioned = legacy.clone().with_source(SourceId::Channel(first.clone()));
    assert!(!metadata(first.clone()).confirms(&unversioned));
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

#[test]
fn shared_quota_is_required_enabled_and_frozen_for_each_channel_candidate() {
    use gateway_core::routing::{
        ConfigRevision, ProviderKind, PublicModelId, RoutingContext, RuntimeSnapshot,
        source::QuotaScopePolicy,
    };
    let quota_id = QuotaScopeId::new("quota_project").expect("quota");
    let quota = QuotaScopePolicy::new(
        quota_id.clone(),
        true,
        RateLimits {
            max_concurrency: 2,
            requests_per_minute: 30,
        },
    )
    .expect("quota policy");
    let base = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        ["chan_a", "chan_b"]
            .into_iter()
            .map(|id| {
                super::model("openai", "shared", super::capabilities())
                    .with_channel(binding(ChannelId::new(id).expect("channel")))
            })
            .collect(),
        Vec::new(),
    )
    .expect("snapshot")
    .with_source_policies(
        ["chan_a", "chan_b"]
            .into_iter()
            .map(|id| {
                SourcePolicy::new(
                    SourceId::Channel(ChannelId::new(id).expect("channel")),
                    true,
                    SourcePreference::default(),
                    RateLimits {
                        max_concurrency: 7,
                        requests_per_minute: 60,
                    },
                    Some(quota_id.clone()),
                )
                .expect("source")
            })
            .collect(),
    )
    .expect("policies");
    let model = PublicModelId::new("shared").expect("model");
    let allowed = channel_access(&["chan_a", "chan_b"]);
    let plan = |snapshot: &RuntimeSnapshot| {
        snapshot.plan_channels(
            &model,
            &super::operation(),
            snapshot.all_account_scope(),
            &RoutingContext::default(),
            &allowed,
        )
    };
    assert!(plan(&base).is_err(), "missing quota cannot mean unlimited");
    let enabled = base
        .with_quota_policies(vec![quota.clone()])
        .expect("enabled");
    let frozen = plan(&enabled).expect("shared quota routes");
    assert_eq!(frozen.candidates().len(), 2);
    for candidate in frozen.candidates() {
        assert_eq!(candidate.shared_quota(), Some(&quota));
        assert_eq!(candidate.source_controls().limits().max_concurrency, 7);
    }
    let healthy = enabled
        .plan_channels(
            &model,
            &super::operation(),
            enabled.all_account_scope(),
            &RoutingContext {
                blocked_sources: std::collections::BTreeSet::from([SourceId::Channel(
                    ChannelId::new("chan_a").expect("channel"),
                )]),
                ..RoutingContext::default()
            },
            &allowed,
        )
        .expect("B keeps its independent health despite sharing quota");
    assert_eq!(healthy.candidates().len(), 1);
    assert_eq!(
        healthy.candidates()[0].source(),
        Some(&SourceId::Channel(ChannelId::new("chan_b").expect("B")))
    );
    assert_eq!(
        frozen.candidates().len(),
        2,
        "a health observation cannot mutate an active plan"
    );
    let changed = enabled
        .clone()
        .with_quota_policies(vec![
            QuotaScopePolicy::new(
                quota_id.clone(),
                true,
                RateLimits {
                    max_concurrency: 5,
                    requests_per_minute: 90,
                },
            )
            .expect("new quota"),
        ])
        .expect("changed");
    assert_eq!(
        plan(&changed).expect("new plan").candidates()[0]
            .shared_quota()
            .expect("quota")
            .limits()
            .max_concurrency,
        5
    );
    assert_eq!(
        frozen.candidates()[0]
            .shared_quota()
            .expect("old quota")
            .limits()
            .max_concurrency,
        2
    );
    let disabled = enabled
        .clone()
        .with_quota_policies(vec![
            QuotaScopePolicy::new(quota_id, false, RateLimits::unlimited()).expect("disabled"),
        ])
        .expect("disabled snapshot");
    assert!(plan(&disabled).is_err());
    assert!(disabled.public_models_for_channels(&allowed).is_empty());
    assert!(
        enabled
            .with_quota_policies(vec![quota.clone(), quota])
            .is_err(),
        "duplicates rejected"
    );
}

#[test]
fn group_preferences_freeze_order_weights_capacity_policy_and_source_limits() {
    use gateway_core::{
        policy::{AccessGroupId, AccessGroupPolicy, AccessGroupRouting},
        routing::source::SourcePreferenceOverride,
        routing::{
            ConfigRevision, ProviderKind, PublicModelId, RoutingContext, RuntimeSnapshot,
            SourceRoutingTarget,
        },
    };
    use std::collections::{BTreeMap, BTreeSet};
    let source = |id: &str| SourceId::Channel(ChannelId::new(id).expect("channel"));
    let limits = RateLimits {
        max_concurrency: 7,
        requests_per_minute: 60,
    };
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).expect("revision"),
        super::scheduling(),
        vec![ProviderKind::new("openai").expect("provider")],
        ["chan_a", "chan_b", "chan_c"]
            .into_iter()
            .map(|id| {
                super::model("openai", "shared", super::capabilities())
                    .with_channel(binding(ChannelId::new(id).expect("channel")))
            })
            .collect(),
        Vec::new(),
    )
    .expect("snapshot")
    .with_source_policies(
        ["chan_a", "chan_b", "chan_c"]
            .into_iter()
            .map(|id| {
                SourcePolicy::new(
                    source(id),
                    true,
                    SourcePreference::new(10, 3).expect("defaults"),
                    limits,
                    None,
                )
                .expect("source")
            })
            .collect(),
    )
    .expect("policies");
    let mut group = AccessGroupPolicy {
        id: AccessGroupId::new("access_preferences").expect("group"),
        enabled: true,
        limits: RateLimits::unlimited(),
        allowed_models: BTreeSet::from(["shared".to_owned()]),
        pool_group_ids: BTreeSet::new(),
        channel_ids: ["chan_a", "chan_b", "chan_c"]
            .into_iter()
            .map(|id| ChannelId::new(id).expect("channel"))
            .collect(),
        routing: AccessGroupRouting {
            allow_capacity_fallback: false,
            source_preferences: BTreeMap::from([
                (
                    source("chan_a"),
                    SourcePreferenceOverride::new(Some(1), None).expect("priority"),
                ),
                (
                    source("chan_b"),
                    SourcePreferenceOverride::new(Some(1), Some(100)).expect("weight"),
                ),
            ]),
        },
    };
    let model = PublicModelId::new("shared").expect("model");
    let plan = |group: &AccessGroupPolicy, seed| {
        snapshot
            .plan_sources(
                SourceRoutingTarget::Model(&model),
                &super::operation(),
                snapshot.all_account_scope(),
                &RoutingContext::default(),
                &AllowedSources::for_access_group(group),
                seed,
            )
            .expect("plan")
    };
    let frozen = plan(&group, 42);
    assert_eq!(frozen.candidates().len(), 3);
    assert_eq!(frozen.candidates()[2].source(), Some(&source("chan_c")));
    assert!(frozen.permits_capacity_fallback(0, 1));
    assert!(!frozen.permits_capacity_fallback(1, 2));
    for candidate in frozen.candidates() {
        assert_eq!(candidate.source_controls().limits(), limits);
    }
    let a = frozen
        .candidates()
        .iter()
        .find(|candidate| candidate.source() == Some(&source("chan_a")))
        .expect("A");
    assert_eq!(a.source_controls().preference().weight(), 3);
    let weighted = (0..1000)
        .filter(|seed| plan(&group, *seed).candidates()[0].source() == Some(&source("chan_b")))
        .count();
    assert!(
        weighted > 900,
        "same-tier weight should dominate: {weighted}"
    );
    group.routing.allow_capacity_fallback = true;
    group.routing.source_preferences.clear();
    assert!(plan(&group, 42).permits_capacity_fallback(1, 2));
    assert!(!frozen.permits_capacity_fallback(1, 2));
    assert_eq!(a.source_controls().preference().priority(), 1);
    group.routing.source_preferences.insert(
        source("chan_hidden"),
        SourcePreferenceOverride::new(Some(1), None).expect("override"),
    );
    assert!(group.validate().is_err());
    assert!(SourcePreferenceOverride::new(None, None).is_err());
    assert!(SourcePreferenceOverride::new(Some(0), None).is_err());
    assert!(SourcePreferenceOverride::new(None, Some(0)).is_err());
}
