//! 请求认证时冻结的账号范围与目录，不依赖路由选择器。

use super::ProviderAccountId;
use crate::identity::ProviderKind;
use crate::validation::{IdentifierError, RoutingError};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

/// `account_groups.id` 的核心值对象。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountGroupId(String);

impl AccountGroupId {
    /// 校验并创建账号分组 ID。
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("grp_") else {
            return Err(IdentifierError::MissingPrefix { expected: "grp_" });
        };
        if suffix.len() != 32
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 快照中一个账号的 Provider 与分组归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAccount {
    provider_kind: ProviderKind,
    group_ids: Arc<BTreeSet<AccountGroupId>>,
}

impl RuntimeAccount {
    #[must_use]
    pub fn new(provider_kind: ProviderKind, group_ids: BTreeSet<AccountGroupId>) -> Self {
        Self {
            provider_kind,
            group_ids: Arc::new(group_ids),
        }
    }

    #[must_use]
    pub const fn provider_kind(&self) -> &ProviderKind {
        &self.provider_kind
    }

    #[must_use]
    pub fn group_ids(&self) -> &BTreeSet<AccountGroupId> {
        &self.group_ids
    }
}

/// 全快照共享的账号、Provider 与分组反向索引。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAccountDirectory {
    accounts: BTreeMap<ProviderAccountId, RuntimeAccount>,
    providers_with_accounts: BTreeSet<ProviderKind>,
    providers_by_group: BTreeMap<AccountGroupId, BTreeSet<ProviderKind>>,
    unpooled: Arc<AccountSubset>,
}

/// 由同一冻结目录派生的账号交集，不能代替 Key 权限。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AccountSubset {
    ids: BTreeSet<ProviderAccountId>,
    providers: BTreeSet<ProviderKind>,
}

impl RuntimeAccountDirectory {
    #[must_use]
    pub fn new(accounts: BTreeMap<ProviderAccountId, RuntimeAccount>) -> Self {
        let mut providers_with_accounts = BTreeSet::new();
        let mut providers_by_group = BTreeMap::<AccountGroupId, BTreeSet<ProviderKind>>::new();
        for account in accounts.values() {
            providers_with_accounts.insert(account.provider_kind.clone());
            for group_id in account.group_ids.iter() {
                providers_by_group
                    .entry(group_id.clone())
                    .or_default()
                    .insert(account.provider_kind.clone());
            }
        }
        let mut unpooled = AccountSubset::default();
        for (id, account) in &accounts {
            if account.group_ids().is_empty() {
                unpooled.ids.insert(id.clone());
                unpooled.providers.insert(account.provider_kind().clone());
            }
        }
        Self {
            accounts,
            providers_with_accounts,
            providers_by_group,
            unpooled: Arc::new(unpooled),
        }
    }

    #[must_use]
    pub fn account(&self, account_id: &ProviderAccountId) -> Option<&RuntimeAccount> {
        self.accounts.get(account_id)
    }

    #[must_use]
    pub fn providers_with_accounts(&self) -> &BTreeSet<ProviderKind> {
        &self.providers_with_accounts
    }

    #[must_use]
    pub fn providers_for_groups<'a>(
        &self,
        group_ids: impl IntoIterator<Item = &'a AccountGroupId>,
    ) -> BTreeSet<ProviderKind> {
        group_ids
            .into_iter()
            .filter_map(|group_id| self.providers_by_group.get(group_id))
            .flatten()
            .cloned()
            .collect()
    }
}

/// 历史请求保存的账号范围种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountRoutingScopeKind {
    None,
    All,
    Groups,
}

impl AccountRoutingScopeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
            Self::Groups => "groups",
        }
    }
}

/// 请求开始时冻结的分组 ID 与名称。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingGroupSnapshot {
    id: AccountGroupId,
    name: String,
}

impl RoutingGroupSnapshot {
    #[must_use]
    pub fn new(id: AccountGroupId, name: String) -> Self {
        Self { id, name }
    }

    #[must_use]
    pub const fn id(&self) -> &AccountGroupId {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// 请求历史所需的完整、稳定账号范围快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRoutingSnapshot {
    kind: AccountRoutingScopeKind,
    groups: Arc<[RoutingGroupSnapshot]>,
}

impl AccountRoutingSnapshot {
    #[must_use]
    pub fn none() -> Self {
        Self {
            kind: AccountRoutingScopeKind::None,
            groups: Arc::from([]),
        }
    }
    #[must_use]
    pub fn all() -> Self {
        Self {
            kind: AccountRoutingScopeKind::All,
            groups: Arc::from([]),
        }
    }

    #[must_use]
    pub fn groups(groups: Vec<RoutingGroupSnapshot>) -> Self {
        Self {
            kind: AccountRoutingScopeKind::Groups,
            groups: Arc::from(groups),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AccountRoutingScopeKind {
        self.kind
    }

    #[must_use]
    pub fn groups_snapshot(&self) -> &[RoutingGroupSnapshot] {
        &self.groups
    }
}

/// Key 持久 binding 编译出的账号权限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRoutingScope {
    AllAccounts,
    Restricted {
        bound_groups: Arc<[RoutingGroupSnapshot]>,
        enabled_group_ids: Arc<BTreeSet<AccountGroupId>>,
        provider_kinds: Arc<BTreeSet<ProviderKind>>,
    },
}

impl ClientRoutingScope {
    #[must_use]
    pub fn all_accounts() -> Self {
        Self::AllAccounts
    }

    /// 接入分组尚未授权任何号池时的显式空范围。
    #[must_use]
    pub fn no_accounts() -> Self {
        Self::Restricted {
            bound_groups: Arc::from([]),
            enabled_group_ids: Arc::new(BTreeSet::new()),
            provider_kinds: Arc::new(BTreeSet::new()),
        }
    }

    pub fn restricted(
        bound_groups: Vec<RoutingGroupSnapshot>,
        enabled_group_ids: BTreeSet<AccountGroupId>,
        provider_kinds: BTreeSet<ProviderKind>,
    ) -> Result<Self, RoutingError> {
        if bound_groups.is_empty() {
            return Err(RoutingError::InvalidAccountScope);
        }
        Ok(Self::Restricted {
            bound_groups: Arc::from(bound_groups),
            enabled_group_ids: Arc::new(enabled_group_ids),
            provider_kinds: Arc::new(provider_kinds),
        })
    }
}

/// 一次认证随 RuntimeSnapshot 冻结的账号目录与 Key 权限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenAccountScope {
    directory: Arc<RuntimeAccountDirectory>,
    client_scope: ClientRoutingScope,
    subset: Option<Arc<AccountSubset>>,
}

impl FrozenAccountScope {
    /// 本次来源选择只能缩小既有权限，不能借号池绑定扩大账号集合。
    #[must_use]
    pub fn within_group(&self, group: RoutingGroupSnapshot) -> Self {
        let allowed = match &self.client_scope {
            ClientRoutingScope::AllAccounts => true,
            ClientRoutingScope::Restricted {
                enabled_group_ids, ..
            } => enabled_group_ids.contains(group.id()),
        };
        let enabled_group_ids = if allowed {
            BTreeSet::from([group.id().clone()])
        } else {
            BTreeSet::new()
        };
        let provider_kinds = self
            .directory
            .providers_for_groups(&enabled_group_ids)
            .intersection(self.provider_kinds())
            .cloned()
            .collect();
        let scope = Self {
            directory: Arc::clone(&self.directory),
            subset: None,
            client_scope: ClientRoutingScope::Restricted {
                bound_groups: Arc::from([group]),
                enabled_group_ids: Arc::new(enabled_group_ids),
                provider_kinds: Arc::new(provider_kinds),
            },
        };
        match &self.subset {
            Some(subset) => scope.intersect_subset(Arc::clone(subset)),
            None => scope,
        }
    }

    #[must_use]
    pub const fn new(
        directory: Arc<RuntimeAccountDirectory>,
        client_scope: ClientRoutingScope,
    ) -> Self {
        Self {
            directory,
            client_scope,
            subset: None,
        }
    }

    /// 未分池账号才可使用无来源候选；分组受限 Key 永远没有这条后备路径。
    #[must_use]
    pub fn only_unpooled(&self) -> Self {
        let subset = match &self.client_scope {
            ClientRoutingScope::AllAccounts => Arc::clone(&self.directory.unpooled),
            ClientRoutingScope::Restricted { .. } => Arc::new(AccountSubset::default()),
        };
        self.intersect_subset(subset)
    }

    /// 固定账号诊断沿用同一权限与来源过滤，不能把其他池账号作为后备。
    #[must_use]
    pub fn only_account(&self, id: &ProviderAccountId) -> Self {
        let mut subset = AccountSubset::default();
        if self.allows(id)
            && let Some(account) = self.directory.account(id)
        {
            subset.ids.insert(id.clone());
            subset.providers.insert(account.provider_kind().clone());
        }
        self.intersect_subset(Arc::new(subset))
    }

    /// 返回当前账号权限能够使用的号池，来源启停与健康由路由层继续过滤。
    #[must_use]
    pub fn pool_group_ids(&self) -> BTreeSet<AccountGroupId> {
        let authorized = match &self.client_scope {
            ClientRoutingScope::AllAccounts => {
                self.directory.providers_by_group.keys().cloned().collect()
            }
            ClientRoutingScope::Restricted {
                enabled_group_ids, ..
            } => enabled_group_ids.as_ref().clone(),
        };
        let Some(subset) = &self.subset else {
            return authorized;
        };
        subset
            .ids
            .iter()
            .filter_map(|id| self.directory.account(id))
            .flat_map(|account| account.group_ids().iter())
            .filter(|id| authorized.contains(*id))
            .cloned()
            .collect()
    }

    fn intersect_subset(&self, subset: Arc<AccountSubset>) -> Self {
        // 常见的全账号 Key 直接复用快照内的未分池索引，不按请求扫描账号目录。
        let subset = if self.subset.is_none()
            && matches!(self.client_scope, ClientRoutingScope::AllAccounts)
        {
            subset
        } else {
            let mut intersection = AccountSubset::default();
            for id in &subset.ids {
                if self.allows(id)
                    && let Some(account) = self.directory.account(id)
                {
                    intersection.ids.insert(id.clone());
                    intersection
                        .providers
                        .insert(account.provider_kind().clone());
                }
            }
            Arc::new(intersection)
        };
        Self {
            directory: Arc::clone(&self.directory),
            client_scope: self.client_scope.clone(),
            subset: Some(subset),
        }
    }

    #[must_use]
    pub fn allows(&self, account_id: &ProviderAccountId) -> bool {
        if self
            .subset
            .as_ref()
            .is_some_and(|subset| !subset.ids.contains(account_id))
        {
            return false;
        }
        let Some(account) = self.directory.account(account_id) else {
            return false;
        };
        match &self.client_scope {
            ClientRoutingScope::AllAccounts => true,
            ClientRoutingScope::Restricted {
                enabled_group_ids,
                provider_kinds,
                ..
            } => {
                provider_kinds.contains(account.provider_kind())
                    && account
                        .group_ids()
                        .iter()
                        .any(|group_id| enabled_group_ids.contains(group_id))
            }
        }
    }

    #[must_use]
    pub fn provider_kinds(&self) -> &BTreeSet<ProviderKind> {
        if let Some(subset) = &self.subset {
            return &subset.providers;
        }
        match &self.client_scope {
            ClientRoutingScope::AllAccounts => self.directory.providers_with_accounts(),
            ClientRoutingScope::Restricted { provider_kinds, .. } => provider_kinds,
        }
    }

    #[must_use]
    pub fn routing_snapshot(&self) -> AccountRoutingSnapshot {
        match &self.client_scope {
            ClientRoutingScope::AllAccounts => AccountRoutingSnapshot::all(),
            ClientRoutingScope::Restricted { bound_groups, .. } => {
                if bound_groups.is_empty() {
                    AccountRoutingSnapshot::none()
                } else {
                    AccountRoutingSnapshot::groups(bound_groups.to_vec())
                }
            }
        }
    }

    #[must_use]
    pub const fn directory(&self) -> &Arc<RuntimeAccountDirectory> {
        &self.directory
    }
}
