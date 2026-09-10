//! 共享配额、配置版本和审计在同一事务中提交。

use async_trait::async_trait;
use gateway_admin::{
    model::{
        MutationContext, Revision,
        quota_scopes::{
            QuotaScopeChange, QuotaScopeFields, QuotaScopeListQuery, QuotaScopePage,
            QuotaScopeRecord,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, QuotaScopeStore},
};
use gateway_core::{identity::QuotaScopeId, policy::RateLimits};
use sqlx::{PgPool, Row as _};

use super::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction};
use crate::{admin_revision, admin_store_error, mutation_audit};

#[derive(Clone)]
pub struct PgQuotaScopeRepository {
    pool: PgPool,
}

impl PgQuotaScopeRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl QuotaScopeStore for PgQuotaScopeRepository {
    async fn list_quota_scopes(
        &self,
        query: QuotaScopeListQuery,
    ) -> AdminStoreResult<QuotaScopePage> {
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
        let total: i64 = sqlx::query_scalar("select count(*) from upstream_quota_scopes where $1::text is null or strpos(lower(name), lower($1)) > 0")
            .bind(&query.search).fetch_one(&mut *transaction).await.map_err(sql_error)?;
        let rows = sqlx::query("select c.*, ((select count(*) from upstream_channels u where u.quota_scope_id = c.id) + (select count(*) from account_groups g where g.quota_scope_id = c.id)) as source_count from upstream_quota_scopes c where $1::text is null or strpos(lower(c.name), lower($1)) > 0 order by c.created_at desc, c.id desc limit $2 offset $3")
            .bind(query.search).bind(i64::from(query.page_size.get())).bind(i64::from(query.page - 1) * i64::from(query.page_size.get()))
            .fetch_all(&mut *transaction).await.map_err(sql_error)?;
        let items = rows
            .iter()
            .map(|row| {
                Ok(QuotaScopeRecord {
                    id: QuotaScopeId::new(row.try_get::<String, _>("id").map_err(sql_error)?)
                        .map_err(|_| invalid())?,
                    fields: QuotaScopeFields {
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
                    source_count: unsigned(row.try_get("source_count").map_err(sql_error)?)?,
                    created_at: row.try_get("created_at").map_err(sql_error)?,
                    updated_at: row.try_get("updated_at").map_err(sql_error)?,
                })
            })
            .collect::<AdminStoreResult<Vec<_>>>()?;
        transaction.commit().await.map_err(sql_error)?;
        Ok(QuotaScopePage {
            items,
            total: unsigned(total)?,
            config_revision: Revision::new(unsigned(revision)?).map_err(|_| invalid())?,
        })
    }

    async fn change_quota_scope(
        &self,
        change: QuotaScopeChange,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        change.validate().map_err(|_| invalid())?;
        let (action, entity, fields) = match &change {
            QuotaScopeChange::Create { .. } => (
                "create",
                "quota_scope",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                ],
            ),
            QuotaScopeChange::Update { .. } => (
                "update",
                "quota_scope",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                ],
            ),
            QuotaScopeChange::Delete { .. } => ("delete", "quota_scope", vec![]),
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
            .map_err(|error| admin_store_error("quota_scope", error))?;
        let result = match change {
            QuotaScopeChange::Create { id, fields } => {
                sqlx::query("insert into upstream_quota_scopes (id, name, note, enabled, max_concurrency, requests_per_minute) values ($1, $2, $3, $4, $5, $6)")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .execute(&mut *transaction).await
            }
            QuotaScopeChange::Update { id, fields } => {
                sqlx::query("update upstream_quota_scopes set name = $2, note = $3, enabled = $4, max_concurrency = $5, requests_per_minute = $6, updated_at = now() where id = $1")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .execute(&mut *transaction).await
            }
            QuotaScopeChange::Delete { id } => sqlx::query("delete from upstream_quota_scopes where id = $1").bind(id.as_str()).execute(&mut *transaction).await,
        }.map_err(sql_error)?;
        if result.rows_affected() != 1 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "quota_scope",
                "共享配额不存在",
            ));
        }
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision)
            .await
            .map_err(|error| admin_store_error("quota_scope", error))?;
        transaction.commit().await.map_err(sql_error)?;
        admin_revision(revision)
    }
}

fn invalid() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Invalid,
        "quota_scope",
        "共享配额字段不合法",
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
        "quota_scope",
        "共享配额操作失败；请检查名称是否重复、配额是否仍被渠道或号池引用",
    )
}
