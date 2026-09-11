//! 接入分组、Key 归属、配置版本和审计在同一事务中提交。

use async_trait::async_trait;
use gateway_admin::{
    model::{
        MutationContext, Revision,
        access_groups::{
            AccessGroupChange, AccessGroupFields, AccessGroupListQuery, AccessGroupPage,
            AccessGroupRecord,
        },
    },
    ports::store::{AccessGroupStore, AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
};
use gateway_core::account::scope::AccountGroupId;
use gateway_core::identity::ChannelId;
use gateway_core::policy::{AccessGroupId, RateLimits};
use gateway_core::{
    policy::AccessGroupRouting,
    routing::source::{SourceId, SourcePreferenceOverride},
};
use sqlx::{PgConnection, PgPool, Row as _};
use std::collections::BTreeMap;

use super::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction};
use crate::{admin_revision, admin_store_error, mutation_audit};

#[derive(Clone)]
pub struct PgAccessGroupRepository {
    pool: PgPool,
}

impl PgAccessGroupRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AccessGroupStore for PgAccessGroupRepository {
    async fn list_access_groups(
        &self,
        query: AccessGroupListQuery,
    ) -> AdminStoreResult<AccessGroupPage> {
        query.validate().map_err(|_| invalid())?;
        let mut transaction = self.pool.begin().await.map_err(sql_error)?;
        sqlx::query("set transaction isolation level repeatable read read only")
            .execute(&mut *transaction)
            .await
            .map_err(sql_error)?;
        let revision: i64 =
            sqlx::query_scalar("select config_revision from runtime_settings where id = 1")
                .fetch_one(&mut *transaction)
                .await
                .map_err(sql_error)?;
        let total: i64 = sqlx::query_scalar("select count(*) from access_groups where $1::text is null or strpos(lower(name), lower($1)) > 0")
            .bind(&query.search).fetch_one(&mut *transaction).await.map_err(sql_error)?;
        let rows = sqlx::query("select c.*, (select count(*) from client_api_keys k where k.access_group_id = c.id) as key_count, array(select p.account_group_id from access_group_pools p where p.access_group_id = c.id order by p.account_group_id) as pool_group_ids, array(select s.channel_id from access_group_channels s where s.access_group_id=c.id order by s.channel_id) as channel_ids from access_groups c where $1::text is null or strpos(lower(c.name), lower($1)) > 0 order by c.created_at desc, c.id desc limit $2 offset $3")
            .bind(query.search).bind(i64::from(query.page_size.get())).bind(i64::from(query.page - 1) * i64::from(query.page_size.get()))
            .fetch_all(&mut *transaction).await.map_err(sql_error)?;
        let ids = rows
            .iter()
            .map(|row| row.try_get::<String, _>("id"))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        let mut routing = load_access_group_routing(&mut transaction, &ids)
            .await
            .map_err(|error| admin_store_error("access_group", error))?;
        let items = rows
            .iter()
            .map(|row| {
                Ok(AccessGroupRecord {
                    id: AccessGroupId::new(row.try_get::<String, _>("id").map_err(sql_error)?)
                        .map_err(|_| invalid())?,
                    fields: AccessGroupFields {
                        routing: routing
                            .remove(&row.try_get::<String, _>("id").map_err(sql_error)?)
                            .ok_or_else(invalid)?,
                        channel_ids: row
                            .try_get::<Vec<String>, _>("channel_ids")
                            .map_err(sql_error)?
                            .into_iter()
                            .map(|id| ChannelId::new(id).map_err(|_| invalid()))
                            .collect::<AdminStoreResult<_>>()?,
                        allowed_models: row
                            .try_get::<Vec<String>, _>("allowed_models")
                            .map_err(sql_error)?
                            .into_iter()
                            .collect(),
                        pool_group_ids: row
                            .try_get::<Vec<String>, _>("pool_group_ids")
                            .map_err(sql_error)?
                            .into_iter()
                            .map(|id| AccountGroupId::new(id).map_err(|_| invalid()))
                            .collect::<AdminStoreResult<_>>()?,
                        name: row.try_get("name").map_err(sql_error)?,
                        note: row.try_get("note").map_err(sql_error)?,
                        enabled: row.try_get("enabled").map_err(sql_error)?,
                        limits: RateLimits {
                            max_concurrency: unsigned(
                                row.try_get("max_concurrency").map_err(sql_error)?,
                            )?,
                            requests_per_minute: unsigned(
                                row.try_get("requests_per_minute").map_err(sql_error)?,
                            )?,
                        },
                    },
                    key_count: unsigned(row.try_get("key_count").map_err(sql_error)?)?,
                    created_at: row.try_get("created_at").map_err(sql_error)?,
                    updated_at: row.try_get("updated_at").map_err(sql_error)?,
                })
            })
            .collect::<AdminStoreResult<Vec<_>>>()?;
        transaction.commit().await.map_err(sql_error)?;
        Ok(AccessGroupPage {
            items,
            total: unsigned(total)?,
            config_revision: Revision::new(unsigned(revision)?).map_err(|_| invalid())?,
        })
    }

    async fn change_access_group(
        &self,
        change: AccessGroupChange,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        change.validate().map_err(|_| invalid())?;
        let source_update = match &change {
            AccessGroupChange::Create { id, fields } | AccessGroupChange::Update { id, fields } => {
                Some((
                    id.clone(),
                    fields.routing.source_preferences.clone(),
                    fields
                        .pool_group_ids
                        .iter()
                        .map(|id| id.as_str().to_owned())
                        .collect::<Vec<_>>(),
                    fields
                        .channel_ids
                        .iter()
                        .map(|id| id.as_str().to_owned())
                        .collect::<Vec<_>>(),
                ))
            }
            _ => None,
        };
        let (action, entity, fields) = match &change {
            AccessGroupChange::Create { .. } => (
                "create",
                "access_group",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                    "allowed_models",
                    "pool_group_ids",
                    "channel_ids",
                    "source_preferences",
                    "allow_capacity_fallback",
                ],
            ),
            AccessGroupChange::Update { .. } => (
                "update",
                "access_group",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                    "allowed_models",
                    "pool_group_ids",
                    "channel_ids",
                    "source_preferences",
                    "allow_capacity_fallback",
                ],
            ),
            AccessGroupChange::Delete { .. } => ("delete", "access_group", vec![]),
            AccessGroupChange::AssignKey { .. } => {
                ("update", "client_api_key", vec!["access_group_id"])
            }
        };
        let audit = mutation_audit(
            context,
            action,
            entity,
            change.entity_ref(),
            fields.into_iter().map(str::to_owned).collect(),
        );
        let mut transaction = self.pool.begin().await.map_err(sql_error)?;
        let revision = bump_config_revision_in_transaction(&mut transaction)
            .await
            .map_err(|error| admin_store_error("access_group", error))?;
        let result = match change {
            AccessGroupChange::Create { id, fields } => {
                sqlx::query("insert into access_groups (id, name, note, enabled, max_concurrency, requests_per_minute, allowed_models, allow_capacity_fallback) values ($1, $2, $3, $4, $5, $6, $7, $8)")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .bind(fields.allowed_models.into_iter().collect::<Vec<_>>())
                    .bind(fields.routing.allow_capacity_fallback)
                    .execute(&mut *transaction).await
            }
            AccessGroupChange::Update { id, fields } => {
                sqlx::query("update access_groups set name = $2, note = $3, enabled = $4, max_concurrency = $5, requests_per_minute = $6, allowed_models = $7, allow_capacity_fallback = $8, updated_at = now() where id = $1")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .bind(fields.allowed_models.into_iter().collect::<Vec<_>>())
                    .bind(fields.routing.allow_capacity_fallback)
                    .execute(&mut *transaction).await
            }
            AccessGroupChange::Delete { id } => sqlx::query("delete from access_groups where id = $1").bind(id.as_str()).execute(&mut *transaction).await,
            AccessGroupChange::AssignKey { key_id, access_group_id } => sqlx::query("update client_api_keys set access_group_id = $2, updated_at = now() where id = $1")
                .bind(key_id.as_str()).bind(access_group_id.as_ref().map(|id| id.as_str())).execute(&mut *transaction).await,
        }.map_err(sql_error)?;
        if result.rows_affected() != 1 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "access_group",
                "接入分组或 Key 不存在",
            ));
        }
        if let Some((id, preferences, pools, channels)) = source_update {
            sqlx::query("delete from access_group_pools where access_group_id = $1")
                .bind(id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(sql_error)?;
            sqlx::query("insert into access_group_pools (access_group_id, account_group_id) select $1, unnest($2::text[])")
                .bind(id.as_str()).bind(pools).execute(&mut *transaction).await.map_err(sql_error)?;
            sqlx::query("delete from access_group_channels where access_group_id=$1")
                .bind(id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(sql_error)?;
            sqlx::query("insert into access_group_channels (access_group_id, channel_id) select $1, unnest($2::text[])").bind(id.as_str()).bind(channels).execute(&mut *transaction).await.map_err(sql_error)?;
            for (source, preference) in preferences {
                let (statement, source_id) = match &source {
                    SourceId::AccountPool(pool) => (
                        "update access_group_pools set priority_override=$3, weight_override=$4 where access_group_id=$1 and account_group_id=$2",
                        pool.as_str(),
                    ),
                    SourceId::Channel(channel) => (
                        "update access_group_channels set priority_override=$3, weight_override=$4 where access_group_id=$1 and channel_id=$2",
                        channel.as_str(),
                    ),
                };
                let updated = sqlx::query(statement)
                    .bind(id.as_str())
                    .bind(source_id)
                    .bind(preference.priority().map(i32::from))
                    .bind(preference.weight().map(i32::from))
                    .execute(&mut *transaction)
                    .await
                    .map_err(sql_error)?;
                if updated.rows_affected() != 1 {
                    return Err(invalid());
                }
            }
        }
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision)
            .await
            .map_err(|error| admin_store_error("access_group", error))?;
        transaction.commit().await.map_err(sql_error)?;
        admin_revision(revision)
    }
}

fn invalid() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Invalid,
        "access_group",
        "接入分组字段不合法",
    )
}
fn unsigned(value: i64) -> AdminStoreResult<u64> {
    u64::try_from(value).map_err(|_| invalid())
}
fn signed(value: u64) -> AdminStoreResult<i64> {
    i64::try_from(value).map_err(|_| invalid())
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
    AdminStoreError::new(
        kind,
        "access_group",
        "接入分组操作失败；请检查名称是否重复、号池是否存在及关联 Key",
    )
}

pub(super) async fn load_access_group_routing(
    connection: &mut PgConnection,
    ids: &[String],
) -> crate::StoreResult<BTreeMap<String, AccessGroupRouting>> {
    let rows = sqlx::query("select ag.id, ag.allow_capacity_fallback, preferences.kind, preferences.source_id, preferences.priority_override, preferences.weight_override from access_groups ag left join lateral (select 'pool' as kind, account_group_id as source_id, priority_override, weight_override from access_group_pools where access_group_id=ag.id and (priority_override is not null or weight_override is not null) union all select 'channel', channel_id, priority_override, weight_override from access_group_channels where access_group_id=ag.id and (priority_override is not null or weight_override is not null)) preferences on true where ag.id=any($1)")
        .bind(ids).fetch_all(connection).await.map_err(|_| crate::postgres_unavailable("load access group routing"))?;
    let mut policies = BTreeMap::new();
    for row in rows {
        let id: String = row.try_get("id").map_err(|_| routing_invalid())?;
        let enabled: bool = row
            .try_get("allow_capacity_fallback")
            .map_err(|_| routing_invalid())?;
        let policy = policies.entry(id).or_insert_with(|| AccessGroupRouting {
            allow_capacity_fallback: enabled,
            source_preferences: BTreeMap::new(),
        });
        let kind: Option<String> = row.try_get("kind").map_err(|_| routing_invalid())?;
        if let Some(kind) = kind {
            let source_id: String = row.try_get("source_id").map_err(|_| routing_invalid())?;
            let source = match kind.as_str() {
                "pool" => SourceId::AccountPool(
                    AccountGroupId::new(source_id).map_err(|_| routing_invalid())?,
                ),
                "channel" => {
                    SourceId::Channel(ChannelId::new(source_id).map_err(|_| routing_invalid())?)
                }
                _ => return Err(routing_invalid()),
            };
            let priority = row
                .try_get::<Option<i32>, _>("priority_override")
                .map_err(|_| routing_invalid())?
                .map(u16::try_from)
                .transpose()
                .map_err(|_| routing_invalid())?;
            let weight = row
                .try_get::<Option<i32>, _>("weight_override")
                .map_err(|_| routing_invalid())?
                .map(u16::try_from)
                .transpose()
                .map_err(|_| routing_invalid())?;
            policy.source_preferences.insert(
                source,
                SourcePreferenceOverride::new(priority, weight).map_err(|_| routing_invalid())?,
            );
        }
    }
    Ok(policies)
}

fn routing_invalid() -> crate::StoreError {
    crate::StoreError::InvalidData {
        entity: "access group routing",
        message: "invalid routing configuration".to_owned(),
    }
}
