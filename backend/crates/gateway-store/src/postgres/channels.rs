//! 渠道配置、版本及安全审计的事务 owner；私密读取供 Provider 调用或编辑合并。

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_admin::{
    model::{
        MutationContext, Revision,
        channels::{ChannelChange, ChannelFields, ChannelListQuery, ChannelPage, ChannelRecord},
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ChannelStore},
};
use gateway_core::{
    channel::{ChannelRevision, ChannelStorePort, ProviderChannelConfig, StoredChannel},
    identity::{ChannelId, ProviderKind, QuotaScopeId},
    policy::RateLimits,
    provider_ports::{ProviderStoreError, ProviderStoreErrorKind},
    routing::source::SourcePreference,
};
use serde_json::Value;
use sqlx::{PgPool, Row as _, postgres::PgRow};

use super::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction};
use crate::{admin_revision, admin_store_error, mutation_audit};

const MAX_PROVIDER_CHANNELS: usize = 1_000;

#[derive(Clone)]
pub struct PgChannelRepository {
    pool: PgPool,
}

impl PgChannelRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ChannelStore for PgChannelRepository {
    async fn load_channel_for_edit(
        &self,
        id: &ChannelId,
    ) -> AdminStoreResult<Option<StoredChannel>> {
        let row = sqlx::query("select id, provider_kind, connection_revision, provider_config_json from upstream_channels where id=$1")
            .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(sql_error)?;
        row.as_ref()
            .map(|row| stored_record(row).map_err(|_| invalid()))
            .transpose()
    }

    async fn list_channels(&self, query: ChannelListQuery) -> AdminStoreResult<ChannelPage> {
        query.validate().map_err(|_| invalid())?;
        let mut tx = self.pool.begin().await.map_err(sql_error)?;
        sqlx::query("set transaction isolation level repeatable read read only")
            .execute(&mut *tx)
            .await
            .map_err(sql_error)?;
        let revision: i64 =
            sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
                .fetch_one(&mut *tx)
                .await
                .map_err(sql_error)?;
        let total: i64 = sqlx::query_scalar("select count(*) from upstream_channels where ($1::text is null or strpos(lower(name), lower($1)) > 0) and ($2::text is null or provider_kind = $2)")
            .bind(&query.search).bind(query.provider.as_ref().map(ProviderKind::as_str)).fetch_one(&mut *tx).await.map_err(sql_error)?;
        let rows = sqlx::query("select id, provider_kind, name, note, enabled, priority, weight, max_concurrency, requests_per_minute, quota_scope_id, connection_revision, created_at, updated_at from upstream_channels where ($1::text is null or strpos(lower(name), lower($1)) > 0) and ($2::text is null or provider_kind = $2) order by created_at desc, id desc limit $3 offset $4")
            .bind(query.search).bind(query.provider.as_ref().map(ProviderKind::as_str)).bind(i64::from(query.page_size.get())).bind(i64::from(query.page - 1) * i64::from(query.page_size.get()))
            .fetch_all(&mut *tx).await.map_err(sql_error)?;
        let items = rows
            .iter()
            .map(public_record)
            .collect::<AdminStoreResult<_>>()?;
        tx.commit().await.map_err(sql_error)?;
        Ok(ChannelPage {
            items,
            total: unsigned(total)?,
            config_revision: Revision::new(unsigned(revision)?).map_err(|_| invalid())?,
        })
    }

    async fn change_channel(
        &self,
        change: ChannelChange,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        change.validate().map_err(|_| invalid())?;
        let id = change.id().clone();
        let (action, changed_fields) = match &change {
            ChannelChange::Create { .. } => (
                "create",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "provider_kind",
                    "priority",
                    "weight",
                    "max_concurrency",
                    "requests_per_minute",
                    "quota_scope_id",
                    "connection",
                ],
            ),
            ChannelChange::Update {
                replacement_config, ..
            } => {
                let mut fields = vec![
                    "name",
                    "note",
                    "enabled",
                    "priority",
                    "weight",
                    "max_concurrency",
                    "requests_per_minute",
                    "quota_scope_id",
                ];
                if replacement_config.is_some() {
                    fields.push("connection");
                }
                ("update", fields)
            }
            ChannelChange::Delete { .. } => ("delete", Vec::new()),
        };
        let audit = mutation_audit(
            context,
            action,
            "channel",
            id.as_str(),
            changed_fields.into_iter().map(str::to_owned).collect(),
        );
        let mut tx = self.pool.begin().await.map_err(sql_error)?;
        let revision = bump_config_revision_in_transaction(&mut tx)
            .await
            .map_err(|error| admin_store_error("channel", error))?;
        let changed = match change {
            ChannelChange::Create { id, provider, fields, config } => sqlx::query("insert into upstream_channels (id, provider_kind, name, note, enabled, priority, weight, max_concurrency, requests_per_minute, quota_scope_id, provider_config_json) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
                .bind(id.as_str()).bind(provider.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled)
                .bind(i32::from(fields.preference.priority())).bind(i32::from(fields.preference.weight())).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                .bind(fields.quota_scope_id.as_ref().map(QuotaScopeId::as_str)).bind(Value::Object(config.into_inner())).execute(&mut *tx).await,
            ChannelChange::Update { id, expected_revision, fields, replacement_config } => sqlx::query("update upstream_channels set name=$3, note=$4, enabled=$5, priority=$6, weight=$7, max_concurrency=$8, requests_per_minute=$9, quota_scope_id=$10, provider_config_json=coalesce($11,provider_config_json), connection_revision=connection_revision+1, updated_at=now() where id=$1 and connection_revision=$2")
                .bind(id.as_str()).bind(signed(expected_revision.get())?).bind(fields.name).bind(fields.note).bind(fields.enabled)
                .bind(i32::from(fields.preference.priority())).bind(i32::from(fields.preference.weight())).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                .bind(fields.quota_scope_id.as_ref().map(QuotaScopeId::as_str)).bind(replacement_config.map(|config| Value::Object(config.into_inner()))).execute(&mut *tx).await,
            ChannelChange::Delete { id, expected_revision } => sqlx::query("delete from upstream_channels where id=$1 and connection_revision=$2")
                .bind(id.as_str()).bind(signed(expected_revision.get())?).execute(&mut *tx).await,
        }.map_err(sql_error)?;
        if changed.rows_affected() == 0 {
            let exists: bool =
                sqlx::query_scalar("select exists(select 1 from upstream_channels where id=$1)")
                    .bind(id.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(sql_error)?;
            return Err(AdminStoreError::new(
                if exists {
                    AdminStoreErrorKind::StaleRevision
                } else {
                    AdminStoreErrorKind::NotFound
                },
                "channel",
                "渠道已变更或不存在，请重新加载",
            ));
        }
        append_admin_audit_event_in_transaction(&mut tx, audit, revision)
            .await
            .map_err(|error| admin_store_error("channel", error))?;
        tx.commit().await.map_err(sql_error)?;
        admin_revision(revision)
    }
}

impl ChannelStorePort for PgChannelRepository {
    fn load_channel<'a>(
        &'a self,
        id: &'a ChannelId,
        expected_revision: ChannelRevision,
    ) -> BoxFuture<'a, Result<Option<StoredChannel>, ProviderStoreError>> {
        Box::pin(async move {
            let row = sqlx::query("select id, provider_kind, connection_revision, provider_config_json from upstream_channels where id=$1 and enabled")
                .bind(id.as_str()).fetch_optional(&self.pool).await.map_err(|_| provider_error(ProviderStoreErrorKind::Unavailable))?;
            let Some(row) = row else {
                return Ok(None);
            };
            let channel = stored_record(&row)?;
            if channel.revision != expected_revision {
                return Err(provider_error(ProviderStoreErrorKind::Conflict));
            }
            Ok(Some(channel))
        })
    }

    fn list_enabled_channels<'a>(
        &'a self,
        provider: &'a ProviderKind,
    ) -> BoxFuture<'a, Result<Vec<StoredChannel>, ProviderStoreError>> {
        Box::pin(async move {
            let rows = sqlx::query("select id, provider_kind, connection_revision, provider_config_json from upstream_channels where provider_kind=$1 and enabled order by id limit $2")
                .bind(provider.as_str()).bind(i64::try_from(MAX_PROVIDER_CHANNELS + 1).expect("bounded channel limit")).fetch_all(&self.pool).await.map_err(|_| provider_error(ProviderStoreErrorKind::Unavailable))?;
            if rows.len() > MAX_PROVIDER_CHANNELS {
                return Err(provider_error(ProviderStoreErrorKind::InvalidData));
            }
            rows.iter().map(stored_record).collect()
        })
    }
}

fn public_record(row: &PgRow) -> AdminStoreResult<ChannelRecord> {
    let id = ChannelId::new(row.try_get::<String, _>("id").map_err(sql_error)?)
        .map_err(|_| invalid())?;
    let fields = ChannelFields {
        name: row.try_get("name").map_err(sql_error)?,
        note: row.try_get("note").map_err(sql_error)?,
        enabled: row.try_get("enabled").map_err(sql_error)?,
        preference: SourcePreference::new(
            u16::try_from(row.try_get::<i32, _>("priority").map_err(sql_error)?)
                .map_err(|_| invalid())?,
            u16::try_from(row.try_get::<i32, _>("weight").map_err(sql_error)?)
                .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?,
        limits: RateLimits {
            max_concurrency: unsigned(row.try_get("max_concurrency").map_err(sql_error)?)?,
            requests_per_minute: unsigned(row.try_get("requests_per_minute").map_err(sql_error)?)?,
        },
        quota_scope_id: row
            .try_get::<Option<String>, _>("quota_scope_id")
            .map_err(sql_error)?
            .map(QuotaScopeId::new)
            .transpose()
            .map_err(|_| invalid())?,
    };
    fields.validate(&id).map_err(|_| invalid())?;
    Ok(ChannelRecord {
        id,
        provider: ProviderKind::new(
            row.try_get::<String, _>("provider_kind")
                .map_err(sql_error)?,
        )
        .map_err(|_| invalid())?,
        fields,
        connection_revision: ChannelRevision::new(unsigned(
            row.try_get("connection_revision").map_err(sql_error)?,
        )?)
        .map_err(|_| invalid())?,
        created_at: row.try_get("created_at").map_err(sql_error)?,
        updated_at: row.try_get("updated_at").map_err(sql_error)?,
    })
}

fn stored_record(row: &PgRow) -> Result<StoredChannel, ProviderStoreError> {
    let invalid = || provider_error(ProviderStoreErrorKind::InvalidData);
    let config: Value = row.try_get("provider_config_json").map_err(|_| invalid())?;
    let Value::Object(config) = config else {
        return Err(invalid());
    };
    Ok(StoredChannel {
        id: ChannelId::new(row.try_get::<String, _>("id").map_err(|_| invalid())?)
            .map_err(|_| invalid())?,
        provider: ProviderKind::new(
            row.try_get::<String, _>("provider_kind")
                .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?,
        revision: ChannelRevision::new(
            u64::try_from(
                row.try_get::<i64, _>("connection_revision")
                    .map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?,
        config: ProviderChannelConfig::new(config).map_err(|_| invalid())?,
    })
}

fn invalid() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Invalid, "channel", "渠道字段不合法")
}
fn unsigned(value: i64) -> AdminStoreResult<u64> {
    u64::try_from(value).map_err(|_| invalid())
}
fn signed(value: u64) -> AdminStoreResult<i64> {
    i64::try_from(value).map_err(|_| invalid())
}
fn provider_error(kind: ProviderStoreErrorKind) -> ProviderStoreError {
    ProviderStoreError::new(kind, "channel configuration")
}
fn sql_error(error: sqlx::Error) -> AdminStoreError {
    let kind = match error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
    {
        Some("23505" | "23503" | "23001") => AdminStoreErrorKind::Conflict,
        Some("23514" | "23502") => AdminStoreErrorKind::Invalid,
        _ => AdminStoreErrorKind::Unavailable,
    };
    AdminStoreError::new(kind, "channel", "渠道操作失败，请检查名称和关联配置")
}
