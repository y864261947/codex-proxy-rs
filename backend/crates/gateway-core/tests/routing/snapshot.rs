use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use futures::future::BoxFuture;

use gateway_core::engine::AttemptContext;
use gateway_core::engine::provider::{
    Provider, ProviderCatalogGeneration, ProviderModelCapabilities, ProviderRegistry,
    ProviderRequest, ProviderStream,
};
use gateway_core::error::{ProviderError, ProviderErrorKind};
use gateway_core::operation::OperationKind;
use gateway_core::policy::{ClientApiKeyId, PlaintextClientApiKey, RateLimits};
use gateway_core::routing::snapshot::{
    RuntimeSnapshotCompileError, RuntimeSnapshotCompiler, SnapshotAccountGroupFacts,
    SnapshotAccountGroupMemberFacts, SnapshotClientPolicyFacts, SnapshotFacts,
    SnapshotProviderAccountFacts, SnapshotSettingsFacts, SnapshotStoreError, SnapshotStorePort,
};
use gateway_core::routing::{
    ConfigRevision, ModelCapabilities, ModelPresentation, ProviderKind, PublicModelId,
    UpstreamModelId,
};
use gateway_core::upstream::UpstreamSendState;

#[derive(Clone)]
struct TestSnapshotStore {
    facts: Arc<Mutex<Result<SnapshotFacts, SnapshotStoreError>>>,
    current_revision: Arc<Mutex<Result<ConfigRevision, SnapshotStoreError>>>,
}

impl TestSnapshotStore {
    fn new(facts: Result<SnapshotFacts, SnapshotStoreError>) -> Self {
        let current_revision = facts.as_ref().map(facts_revision).map_err(Clone::clone);
        Self {
            facts: Arc::new(Mutex::new(facts)),
            current_revision: Arc::new(Mutex::new(current_revision)),
        }
    }
}

impl SnapshotStorePort for TestSnapshotStore {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        Box::pin(async move { self.facts.lock().expect("facts lock").clone() })
    }

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async move { self.current_revision.lock().expect("revision lock").clone() })
    }
}

struct PublishingCatalogProvider {
    generation: AtomicU64,
    queries: AtomicUsize,
}

struct UnavailableCatalogProvider;

#[test]
fn pool_source_settings_are_frozen_in_the_compiled_routing_policy() {
    use gateway_core::{
        identity::QuotaScopeId,
        routing::{
            AccountGroupId,
            source::{SourceControls, SourceId, SourcePreference},
        },
    };
    let id = AccountGroupId::new("grp_00000000000000000000000000000019").expect("pool");
    let controls = SourceControls::new(
        SourcePreference::new(3, 7).expect("preference"),
        RateLimits {
            max_concurrency: 5,
            requests_per_minute: 70,
        },
        Some(QuotaScopeId::new("quota_shared").expect("quota")),
    )
    .expect("controls");
    let compile = |controls| {
        let facts = SnapshotFacts::new(
            ConfigRevision::new(1).expect("revision"),
            ConfigRevision::new(1).expect("revision"),
            SnapshotSettingsFacts::new(3, 50, "smart", BTreeMap::new(), None, None),
            Vec::new(),
            vec![
                SnapshotAccountGroupFacts::new(id.clone(), "Named pool".to_owned(), true)
                    .with_controls(controls),
            ],
            Vec::new(),
            Vec::new(),
        );
        block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(facts))),
                ProviderRegistry::new([Arc::new(UnavailableCatalogProvider) as Arc<dyn Provider>])
                    .expect("registry"),
            )
            .compile(),
        )
        .expect("snapshot")
    };
    let old = compile(controls.clone());
    let new = compile(SourceControls::default());
    let source = SourceId::AccountPool(id);
    let old_policy = old.source_policy(&source).expect("old policy");
    assert_eq!(old_policy.effective_preference(None), controls.preference());
    assert_eq!(old_policy.limits(), controls.limits());
    assert_eq!(old_policy.quota_scope_id(), controls.quota_scope_id());
    assert_eq!(old_policy.snapshot().name(), Some("Named pool"));
    assert_eq!(
        new.source_policy(&source).expect("new policy").limits(),
        RateLimits::unlimited()
    );
}

struct ChannelCatalogProvider(gateway_core::channel::ChannelBinding);

#[async_trait]
impl Provider for ChannelCatalogProvider {
    fn name(&self) -> &'static str {
        "alpha"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(0)
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(vec![
            ProviderModelCapabilities::new(
                UpstreamModelId::new("upstream-model").expect("model"),
                super::capabilities(),
            )
            .with_channel(self.0.clone()),
        ])
    }
    async fn execute(
        &self,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            UpstreamSendState::NotSent,
        ))
    }
}

#[test]
fn channel_catalogs_must_match_persisted_provider_and_configuration_revision() {
    use gateway_core::{
        channel::{ChannelBinding, ChannelRevision},
        identity::ChannelId,
        routing::{
            RoutingContext,
            snapshot::SnapshotChannelFacts,
            source::{AllowedSources, SourceId, SourcePolicy, SourcePreference},
        },
    };
    let id = ChannelId::new("chan_snapshot").expect("channel");
    let binding = ChannelBinding::new(id.clone(), ChannelRevision::new(2).expect("revision"));
    let policy = SourcePolicy::new(
        SourceId::Channel(id.clone()),
        true,
        SourcePreference::default(),
        RateLimits::unlimited(),
        None,
    )
    .expect("policy")
    .with_name("Frozen channel".to_owned())
    .expect("name");
    let fact = SnapshotChannelFacts::new(
        binding.clone(),
        ProviderKind::new("alpha").expect("provider"),
        policy.clone(),
    );
    let compile = |channels, reported| {
        let registry =
            ProviderRegistry::new(
                [Arc::new(ChannelCatalogProvider(reported)) as Arc<dyn Provider>],
            )
            .expect("registry");
        block_on(
            RuntimeSnapshotCompiler::new(
                Arc::new(TestSnapshotStore::new(Ok(
                    facts(1, 1).with_channels(channels)
                ))),
                registry,
            )
            .compile(),
        )
    };
    assert_eq!(
        compile(Vec::new(), binding.clone()).expect_err("unregistered channel"),
        RuntimeSnapshotCompileError::CatalogChanged
    );
    assert_eq!(
        compile(
            vec![fact.clone()],
            ChannelBinding::new(id.clone(), ChannelRevision::new(3).expect("rotated"))
        )
        .expect_err("catalog changed during snapshot read"),
        RuntimeSnapshotCompileError::CatalogChanged
    );
    let wrong = SnapshotChannelFacts::new(
        binding.clone(),
        ProviderKind::new("beta").expect("provider"),
        policy,
    );
    assert_eq!(
        compile(vec![wrong], binding.clone()).expect_err("provider mismatch"),
        RuntimeSnapshotCompileError::CatalogChanged
    );
    let snapshot = compile(vec![fact], binding.clone()).expect("matching catalog");
    let allowed = AllowedSources::new([SourceId::Channel(id)]);
    let plan = snapshot
        .plan_channels(
            &PublicModelId::new("public-model").expect("model"),
            &super::operation(),
            snapshot.all_account_scope(),
            &RoutingContext::default(),
            &allowed,
        )
        .expect("channel plan");
    assert_eq!(plan.candidates()[0].channel_binding(), Some(&binding));
    assert_eq!(
        plan.candidates()[0]
            .source_snapshot()
            .expect("source")
            .name(),
        Some("Frozen channel")
    );
}

#[async_trait]
impl Provider for UnavailableCatalogProvider {
    fn name(&self) -> &'static str {
        "alpha"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(0)
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            UpstreamSendState::NotSent,
        ))
    }

    async fn execute(
        &self,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            UpstreamSendState::NotSent,
        ))
    }
}

#[async_trait]
impl Provider for PublishingCatalogProvider {
    fn name(&self) -> &'static str {
        "alpha"
    }

    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::new(self.generation.load(Ordering::SeqCst))
    }

    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        if self.queries.fetch_add(1, Ordering::SeqCst) == 0 {
            self.generation.store(1, Ordering::SeqCst);
        }
        Ok(vec![
            ProviderModelCapabilities::new(
                UpstreamModelId::new("upstream-model").expect("model"),
                ModelCapabilities::new(
                    std::collections::BTreeSet::from([OperationKind::Generate]),
                    None,
                ),
            )
            .with_presentation(ModelPresentation::new(
                Some("Upstream Model".to_owned()),
                None,
            )),
        ])
    }

    async fn execute(
        &self,
        _: ProviderRequest,
        _: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unavailable,
            UpstreamSendState::NotSent,
        ))
    }
}

#[test]
fn compiler_should_reject_revision_changed_during_consistent_read() {
    let facts = facts(1, 2);
    let compiler = compiler(Arc::new(TestSnapshotStore::new(Ok(facts))));

    let error = block_on(compiler.compile()).expect_err("revision drift must fail closed");

    assert_eq!(error, RuntimeSnapshotCompileError::RevisionChanged);
}

#[test]
fn compiler_should_preserve_passthrough_when_provider_catalog_is_unavailable() {
    let providers =
        ProviderRegistry::new([Arc::new(UnavailableCatalogProvider) as Arc<dyn Provider>])
            .expect("provider registry");
    let compiler =
        RuntimeSnapshotCompiler::new(Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))), providers);

    let snapshot = block_on(compiler.compile()).expect("compile snapshot");
    let provider = ProviderKind::new("alpha").expect("provider");

    assert_eq!(snapshot.revision().get(), 3);
    assert!(snapshot.contains_public_model_for_provider(
        &PublicModelId::new("unknown-upstream-model").expect("model"),
        &provider,
    ));
    assert_eq!(snapshot.mapped_model("public-model"), "upstream-model");
    assert_eq!(snapshot.client_policies().count(), 1);
}

#[test]
fn compiler_retries_when_provider_publishes_catalog_during_compilation() {
    let provider = Arc::new(PublishingCatalogProvider {
        generation: AtomicU64::new(0),
        queries: AtomicUsize::new(0),
    });
    let providers =
        ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).expect("provider registry");
    let compiler =
        RuntimeSnapshotCompiler::new(Arc::new(TestSnapshotStore::new(Ok(facts(3, 3)))), providers);

    let snapshot = block_on(compiler.compile()).expect("stable catalog snapshot");

    assert_eq!(provider.queries.load(Ordering::SeqCst), 2);
    assert_eq!(
        snapshot
            .provider_catalog_generations()
            .get(&ProviderKind::new("alpha").expect("provider"))
            .map(|generation| generation.get()),
        Some(1),
    );
    let profiles =
        snapshot.public_model_profiles_for_provider(&ProviderKind::new("alpha").expect("provider"));
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.model().as_str())
            .collect::<Vec<_>>(),
        vec!["public-model", "upstream-model"],
    );
}

#[test]
fn compiler_should_freeze_valid_client_min_versions() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts_with_min_versions(
        1,
        1,
        Some("26.825.6671".to_owned()),
        Some("0.40.0".to_owned()),
    ))));

    let snapshot = block_on(compiler(store).compile()).expect("valid min versions");

    assert_eq!(
        snapshot
            .min_codex_client_versions()
            .desktop()
            .map(ToString::to_string)
            .as_deref(),
        Some("26.825.6671")
    );
    assert_eq!(
        snapshot
            .min_codex_client_versions()
            .cli()
            .map(ToString::to_string)
            .as_deref(),
        Some("0.40.0")
    );
}

#[test]
fn compiler_should_reject_invalid_persisted_client_min_version() {
    let store = Arc::new(TestSnapshotStore::new(Ok(facts_with_min_versions(
        1,
        1,
        None,
        Some("v0.40.0".to_owned()),
    ))));

    assert_eq!(
        block_on(compiler(store).compile()).expect_err("invalid min version"),
        RuntimeSnapshotCompileError::InvalidData
    );
}

fn facts(config_revision: u64, observed_current_revision: u64) -> SnapshotFacts {
    facts_with_min_versions(config_revision, observed_current_revision, None, None)
}

#[test]
fn global_limits_are_frozen_for_every_key_and_invalid_values_fail_closed() {
    use gateway_core::policy::AdmissionScopeId;
    let build = |limits: RateLimits| {
        SnapshotFacts::new(
            revision(1),
            revision(1),
            SnapshotSettingsFacts::new(3, 0, "smart", BTreeMap::new(), None, None)
                .with_global_limits(limits),
            ["first", "second"]
                .into_iter()
                .map(|name| {
                    SnapshotClientPolicyFacts::new(
                        ClientApiKeyId::new(format!("key_{name}")).expect("key"),
                        PlaintextClientApiKey::new(format!("sk_{name}")).expect("secret"),
                        Vec::new(),
                        RateLimits::unlimited(),
                    )
                })
                .collect(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };
    let limits = RateLimits {
        max_concurrency: 4,
        requests_per_minute: 30,
    };
    let snapshot =
        block_on(compiler(Arc::new(TestSnapshotStore::new(Ok(build(limits))))).compile())
            .expect("snapshot");
    for policy in snapshot.client_policies() {
        let scopes = policy.admission_scopes();
        let global = scopes
            .iter()
            .find(|scope| scope.id == AdmissionScopeId::Global)
            .expect("global scope");
        assert_eq!(global.limits, limits);
        assert_eq!(policy.limits(), RateLimits::unlimited());
    }
    assert_eq!(
        block_on(
            compiler(Arc::new(TestSnapshotStore::new(Ok(build(RateLimits {
                max_concurrency: u64::MAX,
                requests_per_minute: 0
            })))))
            .compile()
        )
        .expect_err("invalid global limits"),
        RuntimeSnapshotCompileError::InvalidData
    );
}

fn facts_with_min_versions(
    config_revision: u64,
    observed_current_revision: u64,
    desktop: Option<String>,
    cli: Option<String>,
) -> SnapshotFacts {
    SnapshotFacts::new(
        revision(config_revision),
        revision(observed_current_revision),
        SnapshotSettingsFacts::new(
            3,
            50,
            "smart",
            BTreeMap::from([("public-model".to_owned(), "upstream-model".to_owned())]),
            desktop,
            cli,
        ),
        vec![SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new("key_one").expect("key ID"),
            PlaintextClientApiKey::new("sk_test").expect("plaintext key"),
            Vec::new(),
            RateLimits::unlimited(),
        )],
        Vec::<SnapshotAccountGroupFacts>::new(),
        Vec::<SnapshotProviderAccountFacts>::new(),
        Vec::<SnapshotAccountGroupMemberFacts>::new(),
    )
}

fn facts_revision(facts: &SnapshotFacts) -> ConfigRevision {
    facts.config_revision()
}

fn compiler(store: Arc<dyn SnapshotStorePort>) -> RuntimeSnapshotCompiler {
    RuntimeSnapshotCompiler::new(store, ProviderRegistry::default())
}

#[test]
fn compiled_snapshot_excludes_keys_of_disabled_customers() {
    use gateway_core::policy::{CustomerId, CustomerPolicy};
    let policies = [true, false]
        .into_iter()
        .map(|enabled| {
            SnapshotClientPolicyFacts::new(
                ClientApiKeyId::new(format!("key_{enabled}")).expect("key ID"),
                PlaintextClientApiKey::new(format!("sk_{enabled}")).expect("key"),
                Vec::new(),
                RateLimits::unlimited(),
            )
            .with_customer(Some(CustomerPolicy {
                id: CustomerId::new(format!("cust_{enabled}")).expect("customer ID"),
                enabled,
                limits: RateLimits {
                    max_concurrency: 4,
                    requests_per_minute: 90,
                },
            }))
        })
        .collect();
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 50, "smart", BTreeMap::new(), None, None),
        policies,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let snapshot = block_on(compiler(Arc::new(TestSnapshotStore::new(Ok(facts)))).compile())
        .expect("compile customer policies");
    let policies = snapshot.client_policies().collect::<Vec<_>>();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].key_id().as_str(), "key_true");
    assert_eq!(policies[0].admission_scopes()[1].limits.max_concurrency, 4);
}

fn revision(value: u64) -> ConfigRevision {
    ConfigRevision::new(value).expect("positive revision")
}

#[test]
fn access_groups_freeze_explicit_pools_without_expanding_empty_or_legacy_permissions() {
    use gateway_core::account::{ProviderAccountId, scope::AccountGroupId};
    use gateway_core::policy::{AccessGroupId, AccessGroupPolicy};
    use std::collections::BTreeSet;
    let first = AccountGroupId::new("grp_00000000000000000000000000000001").expect("pool");
    let second = AccountGroupId::new("grp_00000000000000000000000000000002").expect("pool");
    let account_a = ProviderAccountId::new("acct_a").expect("account");
    let account_b = ProviderAccountId::new("acct_b").expect("account");
    let mut policies = Vec::new();
    for label in ["legacy", "selected", "empty", "disabled"] {
        let mut policy = SnapshotClientPolicyFacts::new(
            ClientApiKeyId::new(label).expect("key"),
            PlaintextClientApiKey::new(format!("sk_{label}")).expect("secret"),
            Vec::new(),
            RateLimits::unlimited(),
        );
        if label != "legacy" {
            policy = policy.with_access_group(Some(AccessGroupPolicy {
                id: AccessGroupId::new(format!("access_{label}")).expect("access group"),
                enabled: label != "disabled",
                limits: RateLimits::unlimited(),
                allowed_models: BTreeSet::from(["public-model".to_owned()]),
                channel_ids: std::collections::BTreeSet::new(),
                pool_group_ids: if label == "empty" {
                    BTreeSet::new()
                } else {
                    BTreeSet::from([first.clone()])
                },
            }));
        }
        policies.push(policy);
    }
    let facts = SnapshotFacts::new(
        revision(1),
        revision(1),
        SnapshotSettingsFacts::new(3, 50, "smart", BTreeMap::new(), None, None),
        policies,
        vec![
            SnapshotAccountGroupFacts::new(first.clone(), "First".to_owned(), true),
            SnapshotAccountGroupFacts::new(second.clone(), "Second".to_owned(), true),
        ],
        vec![
            SnapshotProviderAccountFacts::new(account_a.clone(), "alpha"),
            SnapshotProviderAccountFacts::new(account_b.clone(), "alpha"),
        ],
        vec![
            SnapshotAccountGroupMemberFacts::new(first.clone(), account_a.clone()),
            SnapshotAccountGroupMemberFacts::new(second.clone(), account_b.clone()),
        ],
    );
    let providers =
        ProviderRegistry::new([Arc::new(UnavailableCatalogProvider) as Arc<dyn Provider>])
            .expect("providers");
    let snapshot = block_on(
        RuntimeSnapshotCompiler::new(Arc::new(TestSnapshotStore::new(Ok(facts))), providers)
            .compile(),
    )
    .expect("snapshot");
    for (id, name) in [(first, "First"), (second, "Second")] {
        let source = snapshot
            .source_policy(&gateway_core::routing::source::SourceId::AccountPool(id))
            .expect("compiled pool");
        assert!(source.enabled());
        assert_eq!(source.snapshot().name(), Some(name));
    }
    let policies = snapshot
        .client_policies()
        .map(|policy| (policy.key_id().as_str(), policy))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(policies.len(), 3);
    assert!(policies["legacy"].account_scope().allows(&account_a));
    assert!(policies["legacy"].account_scope().allows(&account_b));
    assert!(policies["selected"].account_scope().allows(&account_a));
    assert!(!policies["selected"].account_scope().allows(&account_b));
    assert!(!policies["empty"].account_scope().allows(&account_a));
    assert!(
        policies["empty"]
            .account_scope()
            .provider_kinds()
            .is_empty()
    );
    assert_eq!(
        policies["empty"]
            .account_scope()
            .routing_snapshot()
            .kind()
            .as_str(),
        "none"
    );
    assert!(
        policies["empty"]
            .account_scope()
            .routing_snapshot()
            .groups_snapshot()
            .is_empty()
    );
    assert_eq!(
        policies["selected"]
            .account_scope()
            .routing_snapshot()
            .kind()
            .as_str(),
        "groups"
    );
}
