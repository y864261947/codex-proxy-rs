//! 终态配置表的一致性 `RuntimeSnapshot` 输入读取。

use std::collections::BTreeMap;

use async_trait::async_trait;
use gateway_core::account::ProviderAccountId;
use gateway_core::routing::{
    AccountGroupId, ConfigRevision,
    snapshot::{
        SnapshotAccountGroupFacts, SnapshotAccountGroupMemberFacts, SnapshotClientPolicyFacts,
        SnapshotFacts, SnapshotProviderAccountFacts, SnapshotSettingsFacts, SnapshotStoreError,
        SnapshotStorePort,
    },
};
use sqlx::{PgPool, Postgres, Transaction};

use crate::{Revision, StoreError, StoreResult, postgres_unavailable};

use super::ClientApiKeySnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRuntimeSettings {
    pub global_limits: gateway_core::policy::RateLimits,
    pub refresh_margin_seconds: u64,
    pub refresh_concurrency: u32,
    pub max_concurrent_per_account: u32,
    pub request_interval_ms: u64,
    pub rotation_strategy: String,
    pub model_mappings: BTreeMap<String, String>,
    pub min_codex_desktop_version: Option<String>,
    pub min_codex_cli_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshotData {
    pub channels: Vec<gateway_core::routing::snapshot::SnapshotChannelFacts>,
    pub config_revision: Revision,
    pub observed_current_revision: Revision,
    pub settings: SnapshotRuntimeSettings,
    pub client_api_keys: Vec<ClientApiKeySnapshot>,
    pub account_groups: Vec<SnapshotAccountGroupData>,
    pub provider_accounts: Vec<SnapshotProviderAccountData>,
    pub group_memberships: Vec<SnapshotGroupMembershipData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAccountGroupData {
    pub id: AccountGroupId,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProviderAccountData {
    pub id: String,
    pub provider_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotGroupMembershipData {
    pub group_id: AccountGroupId,
    pub account_id: String,
}

#[async_trait]
pub trait RuntimeSnapshotRepository: Send + Sync {
    async fn load_runtime_snapshot(&self) -> StoreResult<RuntimeSnapshotData>;
    async fn current_config_revision(&self) -> StoreResult<Revision>;
}

#[derive(Clone)]
pub struct PgRuntimeSnapshotRepository {
    pool: PgPool,
}

impl PgRuntimeSnapshotRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RuntimeSnapshotRepository for PgRuntimeSnapshotRepository {
    async fn load_runtime_snapshot(&self) -> StoreResult<RuntimeSnapshotData> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| postgres_unavailable("begin runtime snapshot"))?;
        sqlx::query("set transaction isolation level repeatable read read only")
            .execute(&mut *transaction)
            .await
            .map_err(|_| postgres_unavailable("configure runtime snapshot transaction"))?;

        let (config_revision, settings) = load_settings(&mut transaction).await?;
        let client_api_keys = load_client_keys(&mut transaction).await?;
        let account_groups = load_account_groups(&mut transaction).await?;
        let provider_accounts = load_provider_accounts(&mut transaction).await?;
        let group_memberships = load_group_memberships(&mut transaction).await?;
        let channels = load_channels(&mut transaction).await?;
        transaction
            .commit()
            .await
            .map_err(|_| postgres_unavailable("commit runtime snapshot"))?;

        let observed_current_revision =
            RuntimeSnapshotRepository::current_config_revision(self).await?;
        Ok(RuntimeSnapshotData {
            channels,
            config_revision,
            observed_current_revision,
            settings,
            client_api_keys,
            account_groups,
            provider_accounts,
            group_memberships,
        })
    }

    async fn current_config_revision(&self) -> StoreResult<Revision> {
        let revision = sqlx::query_scalar::<_, i64>(
            "select config_revision from runtime_settings where id = 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("read current config revision"))?
        .ok_or_else(|| StoreError::NotFound {
            entity: "runtime settings",
            id: "1".to_owned(),
        })?;
        revision_from_i64(revision)
    }
}

impl SnapshotStorePort for PgRuntimeSnapshotRepository {
    fn load_snapshot_facts(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>> {
        Box::pin(async move {
            let data = self
                .load_runtime_snapshot()
                .await
                .map_err(|_| SnapshotStoreError::unavailable())?;
            let config_revision = core_revision(data.config_revision)?;
            let observed_current_revision = core_revision(data.observed_current_revision)?;
            let settings = SnapshotSettingsFacts::new(
                data.settings.max_concurrent_per_account,
                data.settings.request_interval_ms,
                data.settings.rotation_strategy,
                data.settings.model_mappings,
                data.settings.min_codex_desktop_version,
                data.settings.min_codex_cli_version,
            )
            .with_global_limits(data.settings.global_limits);
            let client_policies = data
                .client_api_keys
                .into_iter()
                .map(|key| {
                    SnapshotClientPolicyFacts::new(
                        key.id,
                        key.plaintext_key,
                        key.group_ids,
                        key.limits,
                    )
                    .with_customer(key.customer)
                    .with_access_group(key.access_group)
                })
                .collect();
            let account_groups = data
                .account_groups
                .into_iter()
                .map(|group| SnapshotAccountGroupFacts::new(group.id, group.name, group.enabled))
                .collect();
            let provider_accounts = data
                .provider_accounts
                .into_iter()
                .map(|account| {
                    ProviderAccountId::new(account.id)
                        .map(|id| SnapshotProviderAccountFacts::new(id, account.provider_kind))
                        .map_err(|_| SnapshotStoreError::unavailable())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let group_memberships = data
                .group_memberships
                .into_iter()
                .map(|membership| {
                    ProviderAccountId::new(membership.account_id)
                        .map(|account_id| {
                            SnapshotAccountGroupMemberFacts::new(membership.group_id, account_id)
                        })
                        .map_err(|_| SnapshotStoreError::unavailable())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SnapshotFacts::new(
                config_revision,
                observed_current_revision,
                settings,
                client_policies,
                account_groups,
                provider_accounts,
                group_memberships,
            )
            .with_channels(data.channels))
        })
    }

    fn current_config_revision(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>> {
        Box::pin(async move {
            RuntimeSnapshotRepository::current_config_revision(self)
                .await
                .map_err(|_| SnapshotStoreError::unavailable())
                .and_then(core_revision)
        })
    }
}

fn core_revision(revision: Revision) -> Result<ConfigRevision, SnapshotStoreError> {
    ConfigRevision::new(revision.get()).map_err(|_| SnapshotStoreError::unavailable())
}

async fn load_channels(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Vec<gateway_core::routing::snapshot::SnapshotChannelFacts>> {
    use gateway_core::{
        channel::ChannelBinding,
        routing::{
            snapshot::SnapshotChannelFacts,
            source::{SourceId, SourcePolicy},
        },
    };
    let rows = sqlx::query("select id, provider_kind, name, note, enabled, priority, weight, max_concurrency, requests_per_minute, quota_scope_id, connection_revision, created_at, updated_at from upstream_channels order by id")
        .fetch_all(&mut **transaction).await.map_err(|_| postgres_unavailable("load channel snapshot"))?;
    rows.iter()
        .map(|row| {
            let record = super::channels::public_record(row)
                .map_err(|_| postgres_unavailable("decode channel snapshot"))?;
            let policy = SourcePolicy::new(
                SourceId::Channel(record.id.clone()),
                record.fields.enabled,
                record.fields.preference,
                record.fields.limits,
                record.fields.quota_scope_id,
            )
            .and_then(|policy| policy.with_name(record.fields.name))
            .map_err(|_| postgres_unavailable("decode channel policy"))?;
            Ok(SnapshotChannelFacts::new(
                ChannelBinding::new(record.id, record.connection_revision),
                record.provider,
                policy,
            ))
        })
        .collect()
}

async fn load_settings(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<(Revision, SnapshotRuntimeSettings)> {
    let row = sqlx::query_as::<
        _,
        (
            i64,
            i64,
            i64,
            i64,
            i64,
            String,
            sqlx::types::Json<BTreeMap<String, String>>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        "select config_revision, refresh_margin_seconds, refresh_concurrency,
                max_concurrent_per_account, request_interval_ms, rotation_strategy,
                model_mappings_json, min_codex_desktop_version,
                min_codex_cli_version, global_max_concurrency, global_requests_per_minute
         from runtime_settings where id = 1",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load snapshot settings"))?
    .ok_or_else(|| StoreError::NotFound {
        entity: "runtime settings",
        id: "1".to_owned(),
    })?;
    Ok((
        revision_from_i64(row.0)?,
        SnapshotRuntimeSettings {
            global_limits: gateway_core::policy::RateLimits {
                max_concurrency: to_u64(row.9)?,
                requests_per_minute: to_u64(row.10)?,
            },
            refresh_margin_seconds: to_u64(row.1)?,
            refresh_concurrency: to_u32(row.2)?,
            max_concurrent_per_account: to_u32(row.3)?,
            request_interval_ms: to_u64(row.4)?,
            rotation_strategy: row.5,
            model_mappings: row.6.0,
            min_codex_desktop_version: row.7,
            min_codex_cli_version: row.8,
        },
    ))
}

async fn load_client_keys(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Vec<ClientApiKeySnapshot>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            Vec<String>,
            i64,
            i64,
            Option<String>,
            Option<bool>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<bool>,
            Option<i64>,
            Option<i64>,
            Option<Vec<String>>,
            Vec<String>,
        ),
    >(
        "select k.id, k.key,
                coalesce(array_agg(kg.account_group_id order by kg.account_group_id)
                  filter (where kg.account_group_id is not null), '{}') as group_ids,
                k.max_concurrency, k.requests_per_minute,
                c.id, c.enabled, c.max_concurrency, c.requests_per_minute,
                a.id, a.enabled, a.max_concurrency, a.requests_per_minute, a.allowed_models,
                array(select p.account_group_id from access_group_pools p
                      where p.access_group_id = a.id order by p.account_group_id)
         from client_api_keys k
         left join client_api_key_groups kg on kg.client_api_key_id = k.id
         left join customers c on c.id = k.customer_id
         left join access_groups a on a.id = k.access_group_id
         where k.enabled
         group by k.id, c.id, a.id
         order by k.id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load snapshot client policies"))?;
    rows.into_iter()
        .map(|row| {
            let mut key = ClientApiKeySnapshot::from_persisted(row.0, row.1, row.2, row.3, row.4)?;
            key.customer = row
                .5
                .map(|id| {
                    Ok::<_, StoreError>(gateway_core::policy::CustomerPolicy {
                        id: gateway_core::policy::CustomerId::new(id)
                            .map_err(|_| invalid("invalid customer ID"))?,
                        enabled: row.6.ok_or_else(|| invalid("missing customer status"))?,
                        limits: gateway_core::policy::RateLimits {
                            max_concurrency: to_u64(
                                row.7
                                    .ok_or_else(|| invalid("missing customer concurrency"))?,
                            )?,
                            requests_per_minute: to_u64(
                                row.8.ok_or_else(|| invalid("missing customer RPM"))?,
                            )?,
                        },
                    })
                })
                .transpose()?;
            key.access_group = row
                .9
                .map(|id| {
                    let group = gateway_core::policy::AccessGroupPolicy {
                        id: gateway_core::policy::AccessGroupId::new(id)
                            .map_err(|_| invalid("invalid access group ID"))?,
                        enabled: row
                            .10
                            .ok_or_else(|| invalid("missing access group state"))?,
                        limits: gateway_core::policy::RateLimits {
                            max_concurrency: to_u64(
                                row.11
                                    .ok_or_else(|| invalid("missing access group concurrency"))?,
                            )?,
                            requests_per_minute: to_u64(
                                row.12.ok_or_else(|| invalid("missing access group RPM"))?,
                            )?,
                        },
                        allowed_models: row
                            .13
                            .ok_or_else(|| invalid("missing access group models"))?
                            .into_iter()
                            .collect(),
                        pool_group_ids: row
                            .14
                            .into_iter()
                            .map(|id| {
                                AccountGroupId::new(id)
                                    .map_err(|_| invalid("invalid access pool ID"))
                            })
                            .collect::<StoreResult<_>>()?,
                    };
                    group
                        .validate()
                        .map_err(|_| invalid("invalid access group policy"))?;
                    Ok::<_, StoreError>(group)
                })
                .transpose()?;
            Ok(key)
        })
        .collect()
}

async fn load_account_groups(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Vec<SnapshotAccountGroupData>> {
    let rows = sqlx::query_as::<_, (String, String, bool)>(
        "select id, name, enabled from account_groups order by id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load snapshot account groups"))?;
    rows.into_iter()
        .map(|(id, name, enabled)| {
            Ok(SnapshotAccountGroupData {
                id: AccountGroupId::new(id).map_err(|_| invalid("invalid account group id"))?,
                name,
                enabled,
            })
        })
        .collect()
}

async fn load_provider_accounts(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Vec<SnapshotProviderAccountData>> {
    sqlx::query_as::<_, (String, String)>(
        "select id, provider_kind from provider_accounts order by id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load snapshot provider accounts"))
    .map(|rows| {
        rows.into_iter()
            .map(|(id, provider_kind)| SnapshotProviderAccountData { id, provider_kind })
            .collect()
    })
}

async fn load_group_memberships(
    transaction: &mut Transaction<'_, Postgres>,
) -> StoreResult<Vec<SnapshotGroupMembershipData>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "select account_group_id, provider_account_id
         from account_group_accounts order by account_group_id, provider_account_id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| postgres_unavailable("load snapshot group memberships"))?;
    rows.into_iter()
        .map(|(group_id, account_id)| {
            Ok(SnapshotGroupMembershipData {
                group_id: AccountGroupId::new(group_id)
                    .map_err(|_| invalid("invalid membership group id"))?,
                account_id,
            })
        })
        .collect()
}

fn revision_from_i64(value: i64) -> StoreResult<Revision> {
    Revision::new(to_u64(value)?)
}

fn to_u64(value: i64) -> StoreResult<u64> {
    u64::try_from(value).map_err(|_| invalid("numeric snapshot field is negative"))
}

fn to_u32(value: i64) -> StoreResult<u32> {
    u32::try_from(value).map_err(|_| invalid("numeric snapshot field is outside u32"))
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "runtime snapshot",
        message: message.to_owned(),
    }
}
