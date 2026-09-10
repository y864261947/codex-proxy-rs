//! 客户、Key 归属、配置版本和审计在同一事务中提交。

use async_trait::async_trait;
use gateway_admin::{
    model::{
        MutationContext, Revision,
        customers::{
            CustomerChange, CustomerFields, CustomerListQuery, CustomerPage, CustomerRecord,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, CustomerStore},
};
use gateway_core::policy::{CustomerId, RateLimits};
use sqlx::{PgPool, Row as _};

use super::{append_admin_audit_event_in_transaction, bump_config_revision_in_transaction};
use crate::{admin_revision, admin_store_error, mutation_audit};

#[derive(Clone)]
pub struct PgCustomerRepository {
    pool: PgPool,
}

impl PgCustomerRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CustomerStore for PgCustomerRepository {
    async fn list_customers(&self, query: CustomerListQuery) -> AdminStoreResult<CustomerPage> {
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
        let total: i64 = sqlx::query_scalar("select count(*) from customers where $1::text is null or strpos(lower(name), lower($1)) > 0")
            .bind(&query.search).fetch_one(&mut *transaction).await.map_err(sql_error)?;
        let rows = sqlx::query("select c.*, (select count(*) from client_api_keys k where k.customer_id = c.id) as key_count from customers c where $1::text is null or strpos(lower(c.name), lower($1)) > 0 order by c.created_at desc, c.id desc limit $2 offset $3")
            .bind(query.search).bind(i64::from(query.page_size.get())).bind(i64::from(query.page - 1) * i64::from(query.page_size.get()))
            .fetch_all(&mut *transaction).await.map_err(sql_error)?;
        let items = rows
            .iter()
            .map(|row| {
                Ok(CustomerRecord {
                    id: CustomerId::new(row.try_get::<String, _>("id").map_err(sql_error)?)
                        .map_err(|_| invalid())?,
                    fields: CustomerFields {
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
        Ok(CustomerPage {
            items,
            total: unsigned(total)?,
            config_revision: Revision::new(unsigned(revision)?).map_err(|_| invalid())?,
        })
    }

    async fn change_customer(
        &self,
        change: CustomerChange,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        change.validate().map_err(|_| invalid())?;
        let (action, entity, fields) = match &change {
            CustomerChange::Create { .. } => (
                "create",
                "customer",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                ],
            ),
            CustomerChange::Update { .. } => (
                "update",
                "customer",
                vec![
                    "name",
                    "note",
                    "enabled",
                    "max_concurrency",
                    "requests_per_minute",
                ],
            ),
            CustomerChange::Delete { .. } => ("delete", "customer", vec![]),
            CustomerChange::AssignKey { .. } => ("update", "client_api_key", vec!["customer_id"]),
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
            .map_err(|error| admin_store_error("customer", error))?;
        let result = match change {
            CustomerChange::Create { id, fields } => {
                sqlx::query("insert into customers (id, name, note, enabled, max_concurrency, requests_per_minute) values ($1, $2, $3, $4, $5, $6)")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .execute(&mut *transaction).await
            }
            CustomerChange::Update { id, fields } => {
                sqlx::query("update customers set name = $2, note = $3, enabled = $4, max_concurrency = $5, requests_per_minute = $6, updated_at = now() where id = $1")
                    .bind(id.as_str()).bind(fields.name).bind(fields.note).bind(fields.enabled).bind(signed(fields.limits.max_concurrency)?).bind(signed(fields.limits.requests_per_minute)?)
                    .execute(&mut *transaction).await
            }
            CustomerChange::Delete { id } => sqlx::query("delete from customers where id = $1").bind(id.as_str()).execute(&mut *transaction).await,
            CustomerChange::AssignKey { key_id, customer_id } => sqlx::query("update client_api_keys set customer_id = $2, updated_at = now() where id = $1")
                .bind(key_id.as_str()).bind(customer_id.as_ref().map(|id| id.as_str())).execute(&mut *transaction).await,
        }.map_err(sql_error)?;
        if result.rows_affected() != 1 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "customer",
                "客户或 Key 不存在",
            ));
        }
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision)
            .await
            .map_err(|error| admin_store_error("customer", error))?;
        transaction.commit().await.map_err(sql_error)?;
        admin_revision(revision)
    }
}

fn invalid() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Invalid, "customer", "客户字段不合法")
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
        Some("23505" | "23503") => AdminStoreErrorKind::Conflict,
        Some("23514" | "23502") => AdminStoreErrorKind::Invalid,
        _ => AdminStoreErrorKind::Unavailable,
    };
    AdminStoreError::new(
        kind,
        "customer",
        "客户操作失败；请检查名称是否重复、归属是否存在及关联 Key",
    )
}
