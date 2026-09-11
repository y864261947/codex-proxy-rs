//! 来源身份与选择策略；不包含凭据、URL 或具体上游协议。

use std::{collections::BTreeSet, fmt, num::NonZeroU16};

use crate::{
    account::scope::AccountGroupId,
    identity::{ChannelId, QuotaScopeId},
    policy::RateLimits,
    validation::IdentifierError,
};

/// 一次上游尝试所属的实际来源。账号的真实身份独立保存。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    AccountPool(AccountGroupId),
    Channel(ChannelId),
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccountPool(id) => write!(formatter, "pool:{id}"),
            Self::Channel(id) => write!(formatter, "channel:{id}"),
        }
    }
}

impl SourceId {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AccountPool(_) => "pool",
            Self::Channel(_) => "channel",
        }
    }

    #[must_use]
    pub fn reference(&self) -> &str {
        match self {
            Self::AccountPool(id) => id.as_str(),
            Self::Channel(id) => id.as_str(),
        }
    }
}

/// 请求冻结的来源名称；旧配置没有名称时保持未知，以 ID 展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSnapshot {
    id: SourceId,
    name: Option<String>,
}

impl SourceSnapshot {
    pub fn new(id: SourceId, name: Option<String>) -> Result<Self, IdentifierError> {
        if name.as_deref().is_some_and(|name| {
            name.is_empty()
                || name.trim() != name
                || name.chars().count() > 128
                || name.chars().any(char::is_control)
        }) {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self { id, name })
    }

    #[must_use]
    pub const fn unnamed(id: SourceId) -> Self {
        Self { id, name: None }
    }

    #[must_use]
    pub const fn id(&self) -> &SourceId {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePreference {
    priority: NonZeroU16,
    weight: NonZeroU16,
}

impl SourcePreference {
    pub fn new(priority: u16, weight: u16) -> Result<Self, IdentifierError> {
        Ok(Self {
            priority: NonZeroU16::new(priority).ok_or(IdentifierError::InvalidFormat)?,
            weight: NonZeroU16::new(weight).ok_or(IdentifierError::InvalidFormat)?,
        })
    }

    #[must_use]
    pub const fn priority(self) -> u16 {
        self.priority.get()
    }

    #[must_use]
    pub const fn weight(self) -> u16 {
        self.weight.get()
    }
}

impl Default for SourcePreference {
    fn default() -> Self {
        Self {
            priority: NonZeroU16::MIN,
            weight: NonZeroU16::MIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePreferenceOverride {
    priority: Option<NonZeroU16>,
    weight: Option<NonZeroU16>,
}

impl SourcePreferenceOverride {
    pub fn new(priority: Option<u16>, weight: Option<u16>) -> Result<Self, IdentifierError> {
        if priority.is_none() && weight.is_none() {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self {
            priority: priority
                .map(|value| NonZeroU16::new(value).ok_or(IdentifierError::InvalidFormat))
                .transpose()?,
            weight: weight
                .map(|value| NonZeroU16::new(value).ok_or(IdentifierError::InvalidFormat))
                .transpose()?,
        })
    }

    #[must_use]
    pub fn priority(self) -> Option<u16> {
        self.priority.map(NonZeroU16::get)
    }

    #[must_use]
    pub fn weight(self) -> Option<u16> {
        self.weight.map(NonZeroU16::get)
    }

    #[must_use]
    pub fn resolve(self, defaults: SourcePreference) -> SourcePreference {
        SourcePreference {
            priority: self.priority.unwrap_or(defaults.priority),
            weight: self.weight.unwrap_or(defaults.weight),
        }
    }
}

/// 多个来源共享的项目级配额；每次路由与来源限额同时冻结。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaScopePolicy {
    id: QuotaScopeId,
    enabled: bool,
    limits: RateLimits,
}

impl QuotaScopePolicy {
    pub fn new(
        id: QuotaScopeId,
        enabled: bool,
        limits: RateLimits,
    ) -> Result<Self, IdentifierError> {
        if !limits.is_valid() {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self {
            id,
            enabled,
            limits,
        })
    }
    #[must_use]
    pub const fn id(&self) -> &QuotaScopeId {
        &self.id
    }
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
    #[must_use]
    pub const fn limits(&self) -> RateLimits {
        self.limits
    }
}

/// 来源容量与默认选择偏好，不包含身份或启停状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceControls {
    preference: SourcePreference,
    limits: RateLimits,
    quota_scope_id: Option<QuotaScopeId>,
}

impl SourceControls {
    pub fn new(
        preference: SourcePreference,
        limits: RateLimits,
        quota_scope_id: Option<QuotaScopeId>,
    ) -> Result<Self, IdentifierError> {
        if !limits.is_valid() {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self {
            preference,
            limits,
            quota_scope_id,
        })
    }
    #[must_use]
    pub const fn preference(&self) -> SourcePreference {
        self.preference
    }
    #[must_use]
    pub const fn limits(&self) -> RateLimits {
        self.limits
    }
    #[must_use]
    pub const fn quota_scope_id(&self) -> Option<&QuotaScopeId> {
        self.quota_scope_id.as_ref()
    }
}

/// 来源全局容量与默认选择偏好。组内覆盖只能改变偏好，不能改容量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePolicy {
    id: SourceId,
    name: Option<String>,
    enabled: bool,
    controls: SourceControls,
}

impl SourcePolicy {
    pub fn new(
        id: SourceId,
        enabled: bool,
        preference: SourcePreference,
        limits: RateLimits,
        quota_scope_id: Option<QuotaScopeId>,
    ) -> Result<Self, IdentifierError> {
        let controls = SourceControls::new(preference, limits, quota_scope_id)?;
        Ok(Self {
            id,
            name: None,
            enabled,
            controls,
        })
    }

    pub fn with_name(mut self, name: String) -> Result<Self, IdentifierError> {
        self.name = SourceSnapshot::new(self.id.clone(), Some(name))?.name;
        Ok(self)
    }

    #[must_use]
    pub fn snapshot(&self) -> SourceSnapshot {
        SourceSnapshot {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    #[must_use]
    pub const fn id(&self) -> &SourceId {
        &self.id
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn controls(&self) -> &SourceControls {
        &self.controls
    }

    #[must_use]
    pub const fn limits(&self) -> RateLimits {
        self.controls.limits()
    }

    #[must_use]
    pub const fn quota_scope_id(&self) -> Option<&QuotaScopeId> {
        self.controls.quota_scope_id()
    }

    #[must_use]
    pub fn effective_preference(
        &self,
        group_override: Option<SourcePreference>,
    ) -> SourcePreference {
        group_override.unwrap_or(self.controls.preference())
    }
}

/// 去重与关闭空集合语义在 Core 保持一致，不以未选择表示全部来源。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedSources {
    sources: BTreeSet<SourceId>,
    routing: crate::policy::AccessGroupRouting,
}

impl AllowedSources {
    #[must_use]
    pub fn new(sources: impl IntoIterator<Item = SourceId>) -> Self {
        Self {
            sources: sources.into_iter().collect(),
            routing: Default::default(),
        }
    }

    #[must_use]
    pub fn for_access_group(group: &crate::policy::AccessGroupPolicy) -> Self {
        Self {
            sources: group
                .pool_group_ids
                .iter()
                .cloned()
                .map(SourceId::AccountPool)
                .chain(group.channel_ids.iter().cloned().map(SourceId::Channel))
                .collect(),
            routing: group.routing.clone(),
        }
    }

    #[must_use]
    pub const fn allow_capacity_fallback(&self) -> bool {
        self.routing.allow_capacity_fallback
    }

    #[must_use]
    pub fn preference(&self, policy: &SourcePolicy) -> SourcePreference {
        let defaults = policy.controls().preference();
        self.routing
            .source_preferences
            .get(policy.id())
            .map_or(defaults, |value| value.resolve(defaults))
    }

    #[must_use]
    pub fn allows(&self, source: &SourceId) -> bool {
        self.sources.contains(source)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SourceId> {
        self.sources.iter()
    }
}

/// 在同级来源内按权重产生请求冻结的无重复尝试顺序；低数值优先级先用。
pub(crate) fn selection_order(
    mut sources: Vec<(SourceId, SourcePreference)>,
    mut seed: u64,
) -> Vec<SourceId> {
    sources.sort_by_key(|(_, preference)| preference.priority());
    let mut ordered = Vec::with_capacity(sources.len());
    while let Some((_, first)) = sources.first() {
        let priority = first.priority();
        let count = sources
            .iter()
            .take_while(|(_, preference)| preference.priority() == priority)
            .count();
        let total: u64 = sources[..count]
            .iter()
            .map(|(_, preference)| u64::from(preference.weight()))
            .sum();
        // SplitMix64 仅用于调度分布，不承担凭据或安全随机数职责。
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut random = seed;
        random = (random ^ (random >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        random = (random ^ (random >> 27)).wrapping_mul(0x94d049bb133111eb);
        random ^= random >> 31;
        let mut ticket = random % total;
        let mut selected = 0;
        for (index, (_, preference)) in sources[..count].iter().enumerate() {
            let weight = u64::from(preference.weight());
            if ticket < weight {
                selected = index;
                break;
            }
            ticket -= weight;
        }
        ordered.push(sources.remove(selected).0);
    }
    ordered
}
