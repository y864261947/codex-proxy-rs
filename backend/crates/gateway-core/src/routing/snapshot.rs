//! RuntimeSnapshot 事实、编译、原子发布与版本收敛规则。

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::account::{AccountSelectionPolicy, ProviderAccountId, RotationStrategy};
use crate::engine::provider::{ProviderCatalogGeneration, ProviderRegistry};
use crate::operation::Operation;
use crate::policy::{
    ClientApiKeyId, ClientPolicy, CodexClientMinVersions, CodexClientVersion,
    PlaintextClientApiKey, RateLimits,
};
use crate::validation::RoutingError;

use super::{
    AccountGroupId, ClientRoutingScope, ConfigRevision, FrozenAccountScope, ModelCapabilities,
    ProviderCandidate, ProviderKind, ProviderModel, PublicModelId, RoutingContext,
    RoutingGroupSnapshot, RoutingPlan, RuntimeAccount, RuntimeAccountDirectory, UpstreamModelId,
};

const MAXIMUM_CATALOG_STABILITY_ATTEMPTS: usize = 4;

/// Store 在一个一致性读取中提供的调度设置事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSettingsFacts {
    global_limits: RateLimits,
    max_concurrent_per_account: u32,
    request_interval_ms: u64,
    rotation_strategy: String,
    model_mappings: BTreeMap<String, String>,
    min_codex_desktop_version: Option<String>,
    min_codex_cli_version: Option<String>,
}

impl SnapshotSettingsFacts {
    #[must_use]
    pub const fn with_global_limits(mut self, limits: RateLimits) -> Self {
        self.global_limits = limits;
        self
    }

    #[must_use]
    pub fn new(
        max_concurrent_per_account: u32,
        request_interval_ms: u64,
        rotation_strategy: impl Into<String>,
        model_mappings: BTreeMap<String, String>,
        min_codex_desktop_version: Option<String>,
        min_codex_cli_version: Option<String>,
    ) -> Self {
        Self {
            max_concurrent_per_account,
            global_limits: RateLimits::unlimited(),
            request_interval_ms,
            rotation_strategy: rotation_strategy.into(),
            model_mappings,
            min_codex_desktop_version,
            min_codex_cli_version,
        }
    }
}

/// Store 读取到的一个启用 Client API Key 策略事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotClientPolicyFacts {
    customer: Option<crate::policy::CustomerPolicy>,
    access_group: Option<crate::policy::AccessGroupPolicy>,
    key_id: ClientApiKeyId,
    plaintext_key: PlaintextClientApiKey,
    group_ids: Vec<AccountGroupId>,
    limits: RateLimits,
}

impl SnapshotClientPolicyFacts {
    #[must_use]
    pub fn with_access_group(mut self, group: Option<crate::policy::AccessGroupPolicy>) -> Self {
        self.access_group = group;
        self
    }

    #[must_use]
    pub fn with_customer(mut self, customer: Option<crate::policy::CustomerPolicy>) -> Self {
        self.customer = customer;
        self
    }

    #[must_use]
    pub fn new(
        key_id: ClientApiKeyId,
        plaintext_key: PlaintextClientApiKey,
        group_ids: Vec<AccountGroupId>,
        limits: RateLimits,
    ) -> Self {
        Self {
            key_id,
            customer: None,
            access_group: None,
            plaintext_key,
            group_ids,
            limits,
        }
    }
}

/// Store 读取到的账号分组事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAccountGroupFacts {
    id: AccountGroupId,
    name: String,
    enabled: bool,
}

impl SnapshotAccountGroupFacts {
    #[must_use]
    pub fn new(id: AccountGroupId, name: String, enabled: bool) -> Self {
        Self { id, name, enabled }
    }
}

/// Store 读取到的账号及其固有 Provider 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProviderAccountFacts {
    account_id: ProviderAccountId,
    provider_kind: String,
}

impl SnapshotProviderAccountFacts {
    #[must_use]
    pub fn new(account_id: ProviderAccountId, provider_kind: impl Into<String>) -> Self {
        Self {
            account_id,
            provider_kind: provider_kind.into(),
        }
    }
}

/// Store 读取到的一条分组成员关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAccountGroupMemberFacts {
    group_id: AccountGroupId,
    account_id: ProviderAccountId,
}

impl SnapshotAccountGroupMemberFacts {
    #[must_use]
    pub const fn new(group_id: AccountGroupId, account_id: ProviderAccountId) -> Self {
        Self {
            group_id,
            account_id,
        }
    }
}

/// 渠道的公共策略和连接版本；不携带 Provider 凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotChannelFacts {
    binding: crate::channel::ChannelBinding,
    provider: ProviderKind,
    policy: super::source::SourcePolicy,
}

impl SnapshotChannelFacts {
    #[must_use]
    pub const fn new(
        binding: crate::channel::ChannelBinding,
        provider: ProviderKind,
        policy: super::source::SourcePolicy,
    ) -> Self {
        Self {
            binding,
            provider,
            policy,
        }
    }
    #[must_use]
    pub const fn binding(&self) -> &crate::channel::ChannelBinding {
        &self.binding
    }
    #[must_use]
    pub const fn policy(&self) -> &super::source::SourcePolicy {
        &self.policy
    }
}

/// 一次一致性读取产生的全部 RuntimeSnapshot 持久事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFacts {
    channels: Vec<SnapshotChannelFacts>,
    config_revision: ConfigRevision,
    observed_current_revision: ConfigRevision,
    settings: SnapshotSettingsFacts,
    client_policies: Vec<SnapshotClientPolicyFacts>,
    account_groups: Vec<SnapshotAccountGroupFacts>,
    provider_accounts: Vec<SnapshotProviderAccountFacts>,
    group_memberships: Vec<SnapshotAccountGroupMemberFacts>,
}

impl SnapshotFacts {
    #[must_use]
    pub fn new(
        config_revision: ConfigRevision,
        observed_current_revision: ConfigRevision,
        settings: SnapshotSettingsFacts,
        client_policies: Vec<SnapshotClientPolicyFacts>,
        account_groups: Vec<SnapshotAccountGroupFacts>,
        provider_accounts: Vec<SnapshotProviderAccountFacts>,
        group_memberships: Vec<SnapshotAccountGroupMemberFacts>,
    ) -> Self {
        Self {
            channels: Vec::new(),
            config_revision,
            observed_current_revision,
            settings,
            client_policies,
            account_groups,
            provider_accounts,
            group_memberships,
        }
    }

    #[must_use]
    pub fn with_channels(mut self, channels: Vec<SnapshotChannelFacts>) -> Self {
        self.channels = channels;
        self
    }

    #[must_use]
    pub const fn config_revision(&self) -> ConfigRevision {
        self.config_revision
    }

    #[must_use]
    pub const fn observed_current_revision(&self) -> ConfigRevision {
        self.observed_current_revision
    }
}

/// 不泄漏持久化实现细节的 Snapshot store 错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime snapshot store is unavailable")]
pub struct SnapshotStoreError;

impl SnapshotStoreError {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self
    }
}

/// RuntimeSnapshot 持久事实的数据库中立端口。
pub trait SnapshotStorePort: Send + Sync {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>>;

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>>;
}

/// 快照未发布时可安全记录的稳定错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeSnapshotCompileError {
    #[error("runtime snapshot store is unavailable")]
    StoreUnavailable,
    #[error("runtime configuration changed while the snapshot was loading")]
    RevisionChanged,
    #[error("runtime snapshot contains invalid frozen data")]
    InvalidData,
    #[error("provider model catalog changed while the snapshot was compiling")]
    CatalogChanged,
}

/// Store 一致性事实与 Provider 实时目录的唯一快照编译器。
#[derive(Clone)]
pub struct RuntimeSnapshotCompiler {
    store: Arc<dyn SnapshotStorePort>,
    providers: ProviderRegistry,
}

impl RuntimeSnapshotCompiler {
    #[must_use]
    pub const fn new(store: Arc<dyn SnapshotStorePort>, providers: ProviderRegistry) -> Self {
        Self { store, providers }
    }

    pub(crate) fn store(&self) -> Arc<dyn SnapshotStorePort> {
        Arc::clone(&self.store)
    }

    pub(crate) fn provider_catalog_generations(
        &self,
    ) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        self.providers.catalog_generations()
    }

    /// 读取一个 revision，并为已注册 Provider 查询实时模型目录。
    pub async fn compile(&self) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
        for _ in 0..MAXIMUM_CATALOG_STABILITY_ATTEMPTS {
            let catalog_generations = self.providers.catalog_generations();
            let facts = self
                .store
                .load_snapshot_facts()
                .await
                .map_err(|_| RuntimeSnapshotCompileError::StoreUnavailable)?;
            if facts.config_revision != facts.observed_current_revision {
                return Err(RuntimeSnapshotCompileError::RevisionChanged);
            }
            let snapshot = compile_runtime_snapshot(facts, &self.providers).await?;
            let observed_generations = self.providers.catalog_generations();
            if catalog_generations == observed_generations {
                return Ok(snapshot.with_provider_catalog_generations(observed_generations));
            }
        }
        Err(RuntimeSnapshotCompileError::CatalogChanged)
    }
}

async fn compile_runtime_snapshot(
    facts: SnapshotFacts,
    providers: &ProviderRegistry,
) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
    let provider_kinds = providers.provider_kinds().cloned().collect::<Vec<_>>();
    let registered_providers = provider_kinds.iter().cloned().collect::<BTreeSet<_>>();
    let mut channels = BTreeMap::new();
    for channel in facts.channels {
        if channel.policy.id() != &super::source::SourceId::Channel(channel.binding.id().clone())
            || channels
                .insert(channel.binding.id().clone(), channel)
                .is_some()
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
    }

    // 目录查询失败表示未知；查询成功后，即使为空，也必须与“已知缺少模型”区分。
    let mut provider_models = Vec::new();
    let mut known_provider_catalogs = BTreeSet::new();
    for provider in &provider_kinds {
        let Ok(models) = providers.query_model_capabilities(provider).await else {
            continue;
        };
        known_provider_catalogs.insert(provider.clone());
        for model in models {
            if let Some(binding) = model.channel_binding() {
                let Some(channel) = channels.get(binding.id()) else {
                    return Err(RuntimeSnapshotCompileError::CatalogChanged);
                };
                if channel.binding != *binding || channel.provider != *provider {
                    return Err(RuntimeSnapshotCompileError::CatalogChanged);
                }
            }
            let compiled = ProviderModel::new(
                provider.clone(),
                model.upstream_model().clone(),
                model.capabilities().clone(),
            );
            let compiled = match model.channel_binding().cloned() {
                Some(channel) => compiled.with_channel(channel),
                None => compiled,
            };
            let compiled = match model.presentation().cloned() {
                Some(presentation) => compiled.with_presentation(presentation),
                None => compiled,
            };
            provider_models.push(compiled);
        }
    }

    let mut groups = BTreeMap::new();
    for group in facts.account_groups {
        if group.name.trim() != group.name
            || group.name.is_empty()
            || group.name.chars().count() > 100
            || group.name.chars().any(char::is_control)
            || groups.insert(group.id.clone(), group).is_some()
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
    }

    let mut account_groups = BTreeMap::<ProviderAccountId, BTreeSet<AccountGroupId>>::new();
    let mut accounts = BTreeMap::new();
    for account in facts.provider_accounts {
        let provider_kind = ProviderKind::new(account.provider_kind)
            .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?;
        if !registered_providers.contains(&provider_kind)
            || accounts
                .insert(
                    account.account_id.clone(),
                    RuntimeAccount::new(provider_kind, BTreeSet::new()),
                )
                .is_some()
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
        account_groups.insert(account.account_id, BTreeSet::new());
    }
    let mut memberships = BTreeSet::new();
    for membership in facts.group_memberships {
        if !groups.contains_key(&membership.group_id)
            || !accounts.contains_key(&membership.account_id)
            || !memberships.insert((membership.group_id.clone(), membership.account_id.clone()))
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
        account_groups
            .get_mut(&membership.account_id)
            .ok_or(RuntimeSnapshotCompileError::InvalidData)?
            .insert(membership.group_id);
    }
    for (account_id, group_ids) in account_groups {
        let account = accounts
            .get_mut(&account_id)
            .ok_or(RuntimeSnapshotCompileError::InvalidData)?;
        *account = RuntimeAccount::new(account.provider_kind().clone(), group_ids);
    }
    let account_directory = Arc::new(RuntimeAccountDirectory::new(accounts));

    let model_mappings = facts.settings.model_mappings;
    let min_client_versions = CodexClientMinVersions::new(
        facts
            .settings
            .min_codex_desktop_version
            .as_deref()
            .map(CodexClientVersion::parse)
            .transpose()
            .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?,
        facts
            .settings
            .min_codex_cli_version
            .as_deref()
            .map(CodexClientVersion::parse)
            .transpose()
            .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?,
    );
    let rotation_strategy = RotationStrategy::parse(facts.settings.rotation_strategy.as_str())
        .ok_or(RuntimeSnapshotCompileError::InvalidData)?;
    let selection_policy = AccountSelectionPolicy::new(
        rotation_strategy,
        NonZeroU32::new(facts.settings.max_concurrent_per_account)
            .ok_or(RuntimeSnapshotCompileError::InvalidData)?,
        Duration::from_millis(facts.settings.request_interval_ms),
    );
    if !facts.settings.global_limits.is_valid() {
        return Err(RuntimeSnapshotCompileError::InvalidData);
    }
    let mut client_policies = Vec::with_capacity(facts.client_policies.len());
    for mut policy in facts.client_policies {
        if let Some(group) = &policy.access_group {
            group
                .validate()
                .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?;
            policy.group_ids = group.pool_group_ids.iter().cloned().collect();
        }
        let account_scope = if policy.group_ids.is_empty() && policy.access_group.is_some() {
            FrozenAccountScope::new(
                Arc::clone(&account_directory),
                ClientRoutingScope::no_accounts(),
            )
        } else if policy.group_ids.is_empty() {
            FrozenAccountScope::new(
                Arc::clone(&account_directory),
                ClientRoutingScope::all_accounts(),
            )
        } else {
            let mut seen = BTreeSet::new();
            let mut bound_groups = Vec::with_capacity(policy.group_ids.len());
            let mut enabled_group_ids = BTreeSet::new();
            for group_id in policy.group_ids {
                if !seen.insert(group_id.clone()) {
                    return Err(RuntimeSnapshotCompileError::InvalidData);
                }
                let group = groups
                    .get(&group_id)
                    .ok_or(RuntimeSnapshotCompileError::InvalidData)?;
                bound_groups.push(RoutingGroupSnapshot::new(
                    group.id.clone(),
                    group.name.clone(),
                ));
                if group.enabled {
                    enabled_group_ids.insert(group_id);
                }
            }
            bound_groups.sort_by(|left, right| left.id().cmp(right.id()));
            let provider_kinds = account_directory.providers_for_groups(&enabled_group_ids);
            FrozenAccountScope::new(
                Arc::clone(&account_directory),
                ClientRoutingScope::restricted(bound_groups, enabled_group_ids, provider_kinds)
                    .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?,
            )
        };
        client_policies.push(
            ClientPolicy::new(
                policy.key_id,
                policy.plaintext_key,
                Arc::new(account_scope),
                true,
                policy.limits,
            )
            .with_global_limits(facts.settings.global_limits)
            .with_customer(policy.customer)
            .with_access_group(policy.access_group),
        );
    }

    let mut source_policies = groups
        .values()
        .map(|group| {
            super::source::SourcePolicy::new(
                super::source::SourceId::AccountPool(group.id.clone()),
                group.enabled,
                super::source::SourcePreference::default(),
                crate::policy::RateLimits::unlimited(),
                None,
            )
            .and_then(|policy| policy.with_name(group.name.clone()))
            .map_err(|_| RuntimeSnapshotCompileError::InvalidData)
        })
        .collect::<Result<Vec<_>, _>>()?;
    source_policies.extend(channels.into_values().map(|channel| channel.policy));
    RuntimeSnapshot::new(
        facts.config_revision,
        selection_policy,
        provider_kinds,
        provider_models,
        client_policies,
    )
    .and_then(|snapshot| snapshot.with_source_policies(source_policies))
    .map_err(|_| RuntimeSnapshotCompileError::InvalidData)
    .map(|snapshot| {
        snapshot
            .with_model_mappings(model_mappings)
            .with_account_directory(account_directory)
            .with_known_provider_catalogs(known_provider_catalogs)
            .with_min_codex_client_versions(min_client_versions)
    })
}

/// 数据面使用的不可变配置快照。
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    channel_models:
        Arc<BTreeMap<crate::identity::ChannelId, BTreeMap<UpstreamModelId, ProviderModel>>>,
    source_policies: Arc<BTreeMap<super::source::SourceId, super::source::SourcePolicy>>,
    revision: ConfigRevision,
    account_selection_policy: AccountSelectionPolicy,
    providers: Arc<BTreeSet<ProviderKind>>,
    provider_models: Arc<BTreeMap<ProviderKind, BTreeMap<UpstreamModelId, ModelCapabilities>>>,
    provider_model_presentations:
        Arc<BTreeMap<ProviderKind, BTreeMap<UpstreamModelId, super::ModelPresentation>>>,
    model_mappings: Arc<BTreeMap<String, String>>,
    provider_catalog_generations: Arc<BTreeMap<ProviderKind, ProviderCatalogGeneration>>,
    known_provider_catalogs: Arc<BTreeSet<ProviderKind>>,
    account_directory: Arc<RuntimeAccountDirectory>,
    client_policies: Arc<BTreeMap<ClientApiKeyId, ClientPolicy>>,
    min_codex_client_versions: CodexClientMinVersions,
}

impl RuntimeSnapshot {
    /// 校验 Provider、实时模型目录和 Client API Key，并构建快照。
    pub fn new(
        revision: ConfigRevision,
        account_selection_policy: AccountSelectionPolicy,
        providers: Vec<ProviderKind>,
        provider_models: Vec<ProviderModel>,
        client_policies: Vec<ClientPolicy>,
    ) -> Result<Self, RoutingError> {
        let mut provider_set = BTreeSet::new();
        for provider in providers {
            if !provider_set.insert(provider.clone()) {
                return Err(RoutingError::DuplicateEntity {
                    entity: "provider",
                    id: provider.to_string(),
                });
            }
        }

        let mut known_provider_catalogs = BTreeSet::new();
        let mut channel_models =
            BTreeMap::<crate::identity::ChannelId, BTreeMap<UpstreamModelId, ProviderModel>>::new();
        let mut model_map =
            BTreeMap::<ProviderKind, BTreeMap<UpstreamModelId, ModelCapabilities>>::new();
        let mut presentation_map =
            BTreeMap::<ProviderKind, BTreeMap<UpstreamModelId, super::ModelPresentation>>::new();
        for model in provider_models {
            if let Some(channel) = model.channel.clone() {
                let channel_id = channel.id();
                if !provider_set.contains(&model.provider) {
                    return Err(RoutingError::NotFound {
                        entity: "provider",
                        id: model.provider.to_string(),
                    });
                }
                // 成功返回渠道目录不能授权该适配器的未知账号路径。
                known_provider_catalogs.insert(model.provider.clone());
                let models = channel_models.entry(channel_id.clone()).or_default();
                if models.values().any(|existing| {
                    existing.provider != model.provider || existing.channel != model.channel
                }) {
                    return Err(RoutingError::DuplicateEntity {
                        entity: "channel provider or revision",
                        id: channel_id.to_string(),
                    });
                }
                if models.insert(model.upstream_model.clone(), model).is_some() {
                    return Err(RoutingError::DuplicateEntity {
                        entity: "channel model",
                        id: channel_id.to_string(),
                    });
                }
                continue;
            }
            let ProviderModel {
                channel: _,
                provider,
                upstream_model,
                capabilities,
                presentation,
            } = model;
            known_provider_catalogs.insert(provider.clone());
            if !provider_set.contains(&provider) {
                return Err(RoutingError::NotFound {
                    entity: "provider",
                    id: provider.to_string(),
                });
            }
            let models = model_map.entry(provider.clone()).or_default();
            if models
                .insert(upstream_model.clone(), capabilities)
                .is_some()
            {
                return Err(RoutingError::DuplicateEntity {
                    entity: "provider model",
                    id: upstream_model.to_string(),
                });
            }
            if let Some(presentation) = presentation {
                presentation_map
                    .entry(provider)
                    .or_default()
                    .insert(upstream_model, presentation);
            }
        }

        let mut client_policy_map = BTreeMap::new();
        for policy in client_policies {
            let id = policy.key_id().clone();
            if client_policy_map.insert(id.clone(), policy).is_some() {
                return Err(RoutingError::DuplicateEntity {
                    entity: "client API key",
                    id: id.to_string(),
                });
            }
        }
        client_policy_map.retain(|_, policy| policy.enabled());

        Ok(Self {
            channel_models: Arc::new(channel_models),
            source_policies: Arc::new(BTreeMap::new()),
            revision,
            account_selection_policy,
            providers: Arc::new(provider_set),
            provider_models: Arc::new(model_map),
            provider_model_presentations: Arc::new(presentation_map),
            model_mappings: Arc::new(BTreeMap::new()),
            provider_catalog_generations: Arc::new(BTreeMap::new()),
            known_provider_catalogs: Arc::new(known_provider_catalogs),
            account_directory: Arc::new(RuntimeAccountDirectory::default()),
            client_policies: Arc::new(client_policy_map),
            min_codex_client_versions: CodexClientMinVersions::default(),
        })
    }

    /// 发布与模型目录同一请求快照的来源启停、容量与调度策略。
    pub fn with_source_policies(
        mut self,
        policies: Vec<super::source::SourcePolicy>,
    ) -> Result<Self, RoutingError> {
        let mut sources = BTreeMap::new();
        for policy in policies {
            let id = policy.id().clone();
            if sources.insert(id.clone(), policy).is_some() {
                return Err(RoutingError::DuplicateEntity {
                    entity: "source",
                    id: id.to_string(),
                });
            }
        }
        self.source_policies = Arc::new(sources);
        Ok(self)
    }

    #[must_use]
    pub fn source_policy(
        &self,
        source: &super::source::SourceId,
    ) -> Option<&super::source::SourcePolicy> {
        self.source_policies.get(source)
    }

    /// 接入分组选定来源后，号池候选使用该池与请求权限的交集。
    pub fn plan_sources(
        &self,
        target: super::SourceRoutingTarget<'_>,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
        allowed: &super::source::AllowedSources,
        seed: u64,
    ) -> Result<RoutingPlan, RoutingError> {
        let order = super::source::selection_order(
            allowed
                .iter()
                .filter_map(|source| {
                    self.source_policy(source)
                        .filter(|policy| policy.enabled())
                        .map(|policy| (source.clone(), policy.effective_preference(None)))
                })
                .collect(),
            seed,
        );
        let mut candidates = Vec::new();
        for source in order {
            let policy = self
                .source_policy(&source)
                .expect("ordered source belongs to this immutable snapshot");
            let plan = match &source {
                super::source::SourceId::AccountPool(group) => {
                    let scope = Arc::new(
                        account_scope.within_group(RoutingGroupSnapshot::new(
                            group.clone(),
                            policy
                                .snapshot()
                                .name()
                                .unwrap_or(group.as_str())
                                .to_owned(),
                        )),
                    );
                    if scope.provider_kinds().is_empty() {
                        continue;
                    }
                    match target {
                        super::SourceRoutingTarget::Model(model) => {
                            self.plan(model, operation, scope, context)
                        }
                        super::SourceRoutingTarget::ProviderEndpoint(provider) => {
                            self.plan_provider_endpoint(provider, operation, scope, context)
                        }
                    }
                }
                super::source::SourceId::Channel(_) => match target {
                    super::SourceRoutingTarget::Model(model) => self.plan_channels(
                        model,
                        operation,
                        Arc::clone(&account_scope),
                        context,
                        &super::source::AllowedSources::new([source.clone()]),
                    ),
                    super::SourceRoutingTarget::ProviderEndpoint(_) => continue,
                },
            };
            match plan {
                Ok(plan) => {
                    candidates.extend(plan.candidates().iter().cloned().map(|mut candidate| {
                        candidate.source = Some(policy.snapshot());
                        candidate
                    }))
                }
                Err(
                    RoutingError::EmptyAccountScope
                    | RoutingError::NoCapableProvider { .. }
                    | RoutingError::NoCapableProviderEndpoint { .. },
                ) => {}
                Err(error) => return Err(error),
            }
        }
        if candidates.is_empty() {
            return Err(match target {
                super::SourceRoutingTarget::Model(model) => RoutingError::NoCapableProvider {
                    model: model.to_string(),
                },
                super::SourceRoutingTarget::ProviderEndpoint(provider) => {
                    RoutingError::NoCapableProviderEndpoint {
                        provider: provider.to_string(),
                    }
                }
            });
        }
        Ok(RoutingPlan {
            config_revision: self.revision,
            account_selection_policy: self.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("nonzero attempt limit"),
            account_scope,
            candidates: Arc::from(candidates),
        })
    }

    /// 只返回显式允许且启用渠道中真实发现的模型；别名也必须解析到该渠道。
    #[must_use]
    pub fn public_models_for_channels(
        &self,
        allowed: &super::source::AllowedSources,
    ) -> Vec<PublicModelId> {
        let mut visible = BTreeSet::new();
        for source in allowed.iter() {
            let super::source::SourceId::Channel(channel) = source else {
                continue;
            };
            if !self
                .source_policy(source)
                .is_some_and(super::source::SourcePolicy::enabled)
            {
                continue;
            }
            let Some(models) = self.channel_models.get(channel) else {
                continue;
            };
            visible.extend(
                models
                    .keys()
                    .filter_map(|model| PublicModelId::new(model.as_str()).ok()),
            );
            for alias in self.model_mappings.keys() {
                let mapped = self.mapped_model(alias);
                if models.keys().any(|model| model.as_str() == mapped)
                    && let Ok(alias) = PublicModelId::new(alias.clone())
                {
                    visible.insert(alias);
                }
            }
        }
        visible.into_iter().collect()
    }

    /// 渠道必须同时有显式权限、启用策略及该来源自己的能力事实。
    #[must_use]
    pub fn public_model_profiles_for_channels(
        &self,
        allowed: &super::source::AllowedSources,
    ) -> Vec<super::PublicModelProfile> {
        self.public_models_for_channels(allowed)
            .into_iter()
            .filter_map(|public_model| {
                let target = self.mapped_model(public_model.as_str());
                let presentation = allowed.iter().find_map(|source| {
                    let super::source::SourceId::Channel(channel) = source else {
                        return None;
                    };
                    if !self
                        .source_policy(source)
                        .is_some_and(super::source::SourcePolicy::enabled)
                    {
                        return None;
                    }
                    self.channel_models
                        .get(channel)?
                        .values()
                        .find(|model| model.upstream_model.as_str() == target)?
                        .presentation
                        .clone()
                })?;
                Some(super::PublicModelProfile::new(public_model, presentation))
            })
            .collect()
    }

    /// 渠道必须同时有显式权限、启用策略及该来源自己的能力事实。
    /// 账号 scope 保留在计划中用于请求历史，绝不作为渠道授权的替代。
    pub fn plan_channels(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
        allowed: &super::source::AllowedSources,
    ) -> Result<RoutingPlan, RoutingError> {
        let requirements = operation.capability_requirements();
        let mapped = self.mapped_model(public_model.as_str());
        let mut candidates = Vec::new();
        for source in allowed.iter() {
            let super::source::SourceId::Channel(channel) = source else {
                continue;
            };
            if !self
                .source_policy(source)
                .is_some_and(super::source::SourcePolicy::enabled)
            {
                continue;
            }
            let Some(model) = self.channel_models.get(channel).and_then(|models| {
                models
                    .values()
                    .find(|model| model.upstream_model.as_str() == mapped)
            }) else {
                continue;
            };
            if context.blocked_providers.contains(&model.provider)
                || context
                    .required_provider
                    .as_ref()
                    .is_some_and(|required| required != &model.provider)
            {
                continue;
            }
            let Some(emulated_features) = model.capabilities.match_requirements(&requirements)
            else {
                continue;
            };
            candidates.push(ProviderCandidate {
                channel: model.channel.clone(),
                source: self
                    .source_policy(source)
                    .map(super::source::SourcePolicy::snapshot),
                provider: model.provider.clone(),
                upstream_model: Some(model.upstream_model.clone()),
                emulated_features,
                account_scope: Arc::clone(&account_scope),
            });
        }
        if candidates.is_empty() {
            return Err(RoutingError::NoCapableProvider {
                model: public_model.to_string(),
            });
        }
        Ok(RoutingPlan {
            config_revision: self.revision,
            account_selection_policy: self.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("nonzero request attempt limit"),
            account_scope,
            candidates: Arc::from(candidates),
        })
    }

    #[must_use]
    pub fn with_model_mappings(mut self, mappings: BTreeMap<String, String>) -> Self {
        self.model_mappings = Arc::new(mappings);
        self
    }

    #[must_use]
    pub fn with_account_directory(mut self, directory: Arc<RuntimeAccountDirectory>) -> Self {
        self.account_directory = directory;
        self
    }

    #[must_use]
    pub fn with_min_codex_client_versions(mut self, versions: CodexClientMinVersions) -> Self {
        self.min_codex_client_versions = versions;
        self
    }

    #[must_use]
    fn with_known_provider_catalogs(mut self, providers: BTreeSet<ProviderKind>) -> Self {
        self.known_provider_catalogs = Arc::new(providers);
        self
    }

    #[must_use]
    pub fn all_account_scope(&self) -> Arc<FrozenAccountScope> {
        Arc::new(FrozenAccountScope::new(
            Arc::clone(&self.account_directory),
            ClientRoutingScope::all_accounts(),
        ))
    }

    #[must_use]
    fn with_provider_catalog_generations(
        mut self,
        generations: BTreeMap<ProviderKind, ProviderCatalogGeneration>,
    ) -> Self {
        self.provider_catalog_generations = Arc::new(generations);
        self
    }

    #[must_use]
    pub fn provider_catalog_generations(
        &self,
    ) -> &BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        &self.provider_catalog_generations
    }

    #[must_use]
    pub const fn revision(&self) -> ConfigRevision {
        self.revision
    }

    /// 返回目录发现模型与设置映射的并集，仅用于公开模型展示。
    #[must_use]
    pub fn public_models_for_provider(&self, provider: &ProviderKind) -> Vec<PublicModelId> {
        let mut models = BTreeSet::new();
        if let Some(discovered) = self.provider_models.get(provider) {
            models.extend(
                discovered
                    .keys()
                    .filter_map(|model| PublicModelId::new(model.as_str().to_owned()).ok()),
            );
        }
        models.extend(
            self.model_mappings
                .keys()
                .filter_map(|model| PublicModelId::new(model.clone()).ok()),
        );
        models.into_iter().collect()
    }

    /// 返回 Provider 已明确声明画像的公开模型；没有画像时不猜测 Provider 语义。
    #[must_use]
    pub fn public_model_profiles_for_provider(
        &self,
        provider: &ProviderKind,
    ) -> Vec<super::PublicModelProfile> {
        let Some(presentations) = self.provider_model_presentations.get(provider) else {
            return Vec::new();
        };
        let mut profiles = BTreeMap::new();
        for (model, presentation) in presentations {
            if let Ok(public_model) = PublicModelId::new(model.as_str().to_owned()) {
                profiles.insert(public_model, presentation.clone());
            }
        }
        for alias in self.model_mappings.keys() {
            let target = self.mapped_model(alias);
            let Some(presentation) = presentations.iter().find_map(|(model, presentation)| {
                (model.as_str() == target).then_some(presentation)
            }) else {
                continue;
            };
            if let Ok(public_model) = PublicModelId::new(alias.clone()) {
                profiles.insert(public_model, presentation.clone());
            }
        }
        profiles
            .into_iter()
            .map(|(model, presentation)| super::PublicModelProfile::new(model, presentation))
            .collect()
    }

    #[must_use]
    pub fn public_models(&self) -> Vec<PublicModelId> {
        self.providers
            .iter()
            .flat_map(|provider| self.public_models_for_provider(provider))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 合并冻结账号范围内实际存在 Provider 的公开模型。
    #[must_use]
    pub fn public_models_for_scope(&self, scope: &FrozenAccountScope) -> Vec<PublicModelId> {
        scope
            .provider_kinds()
            .iter()
            .flat_map(|provider| self.public_models_for_provider(provider))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 合并冻结账号范围内实际存在 Provider 的公开模型画像。
    #[must_use]
    pub fn public_model_profiles_for_scope(
        &self,
        scope: &FrozenAccountScope,
    ) -> Vec<super::PublicModelProfile> {
        let mut profiles = BTreeMap::new();
        for provider in scope.provider_kinds() {
            for profile in self.public_model_profiles_for_provider(provider) {
                profiles
                    .entry(profile.model().clone())
                    .or_insert_with(|| profile.presentation().clone());
            }
        }
        profiles
            .into_iter()
            .map(|(model, presentation)| super::PublicModelProfile::new(model, presentation))
            .collect()
    }

    /// 已取得目录时以映射后的上游模型为准；目录不可用时由 Provider 在发送前判定。
    #[must_use]
    pub fn contains_public_model_for_provider(
        &self,
        public_model: &PublicModelId,
        provider: &ProviderKind,
    ) -> bool {
        if !self.providers.contains(provider) {
            return false;
        }
        let upstream_model = self.mapped_model(public_model.as_str());
        match self.provider_models.get(provider) {
            Some(models) => models.keys().any(|model| model.as_str() == upstream_model),
            None => !self.known_provider_catalogs.contains(provider),
        }
    }

    #[must_use]
    pub fn contains_public_model_for_scope(
        &self,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
    ) -> bool {
        scope
            .provider_kinds()
            .iter()
            .any(|provider| self.contains_public_model_for_provider(public_model, provider))
    }

    #[must_use]
    pub fn mapped_model(&self, requested: &str) -> String {
        let original = requested;
        let mut current = original.to_owned();
        let mut seen = BTreeSet::new();
        for _ in 0..20 {
            let Some(target) = self.model_mappings.get(&current).map(String::as_str) else {
                return current;
            };
            if !seen.insert(current.clone()) || seen.contains(target) {
                return original.to_owned();
            }
            current = target.to_owned();
        }
        original.to_owned()
    }

    pub fn client_policies(&self) -> impl Iterator<Item = &ClientPolicy> {
        self.client_policies.values()
    }

    #[must_use]
    pub const fn min_codex_client_versions(&self) -> &CodexClientMinVersions {
        &self.min_codex_client_versions
    }

    pub fn plan(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        let requirements = operation.capability_requirements();
        let mut candidates = Vec::new();

        if context.required_provider.is_none() && account_scope.provider_kinds().is_empty() {
            return Err(RoutingError::EmptyAccountScope);
        }

        let providers = context.required_provider.as_ref().map_or_else(
            || account_scope.provider_kinds().clone(),
            |provider| BTreeSet::from([provider.clone()]),
        );
        for provider in &providers {
            if !self.providers.contains(provider) {
                continue;
            }
            if context
                .required_provider
                .as_ref()
                .is_some_and(|expected| expected != provider)
                || context.blocked_providers.contains(provider)
            {
                continue;
            }
            let requested_model = public_model.as_str();
            let mapped_model = self.mapped_model(requested_model);
            let upstream_model = if self.model_mappings.contains_key(requested_model) {
                UpstreamModelId::new(mapped_model)
            } else {
                UpstreamModelId::from_client_wire(mapped_model)
            }
            .map_err(|_| RoutingError::InvalidIdentifier)?;
            let emulated_features = match self
                .provider_models
                .get(provider)
                .and_then(|models| models.get(&upstream_model))
            {
                Some(capabilities) => {
                    let Some(emulated) = capabilities.match_requirements(&requirements) else {
                        continue;
                    };
                    emulated
                }
                None if self.known_provider_catalogs.contains(provider) => continue,
                None => BTreeSet::new(),
            };
            candidates.push(ProviderCandidate {
                channel: None,
                source: None,
                provider: provider.clone(),
                upstream_model: Some(upstream_model),
                emulated_features,
                account_scope: Arc::clone(&account_scope),
            });
        }

        if candidates.is_empty() {
            return Err(RoutingError::NoCapableProvider {
                model: public_model.as_str().to_owned(),
            });
        }

        Ok(RoutingPlan {
            config_revision: self.revision,
            account_selection_policy: self.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("constant request attempt limit is non-zero"),
            account_scope,
            candidates: Arc::from(candidates),
        })
    }

    /// 为 Provider 自有、且不属于文本模型目录的端点冻结请求计划。
    ///
    /// 端点 adapter 已经确定 Provider，因此这里只执行账号范围、circuit 和注册
    /// 状态检查；不会读取业务正文、构造模型或查询 Provider 文本模型目录。
    pub fn plan_provider_endpoint(
        &self,
        provider: &ProviderKind,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        let allowed = !account_scope.provider_kinds().is_empty()
            && account_scope.provider_kinds().contains(provider)
            && self.providers.contains(provider)
            && context
                .required_provider
                .as_ref()
                .is_none_or(|required| required == provider)
            && !context.blocked_providers.contains(provider);
        if !allowed {
            return Err(RoutingError::NoCapableProviderEndpoint {
                provider: provider.as_str().to_owned(),
            });
        }
        let candidate = ProviderCandidate {
            channel: None,
            source: None,
            provider: provider.clone(),
            upstream_model: None,
            emulated_features: BTreeSet::new(),
            account_scope: Arc::clone(&account_scope),
        };
        Ok(RoutingPlan {
            config_revision: self.revision,
            account_selection_policy: self.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("constant request attempt limit is non-zero"),
            account_scope,
            candidates: Arc::from([candidate]),
        })
    }
}
