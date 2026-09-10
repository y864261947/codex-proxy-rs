//! 全站硬限额与配置版本、安全审计一并提交；旧运行设置保存不覆盖这些字段。

use gateway_admin::{
    model::{MutationContext, settings::GlobalAdmissionSettings},
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
};
use gateway_core::policy::RateLimits;

use super::{
    PgControlPlaneRepository, append_admin_audit_event_in_transaction,
    bump_config_revision_in_transaction,
};
use crate::{admin_revision, admin_store_error, mutation_audit};

impl PgControlPlaneRepository {
    pub async fn load_global_admission(&self) -> AdminStoreResult<GlobalAdmissionSettings> {
        let (concurrency, rpm, revision): (i64, i64, i64) = sqlx::query_as(
            "select global_max_concurrency, global_requests_per_minute, config_revision from runtime_settings where id = 1",
        ).fetch_one(&self.pool).await.map_err(|_| unavailable())?;
        let limits = RateLimits {
            max_concurrency: u64::try_from(concurrency).map_err(|_| unavailable())?,
            requests_per_minute: u64::try_from(rpm).map_err(|_| unavailable())?,
        };
        if !limits.is_valid() {
            return Err(unavailable());
        }
        Ok(GlobalAdmissionSettings {
            limits,
            config_revision: gateway_admin::model::Revision::new(
                u64::try_from(revision).map_err(|_| unavailable())?,
            )
            .map_err(|_| unavailable())?,
        })
    }

    pub async fn replace_global_admission(
        &self,
        limits: RateLimits,
        context: &MutationContext,
    ) -> AdminStoreResult<GlobalAdmissionSettings> {
        if !limits.is_valid() {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "global admission",
                "全站限额超出有效范围",
            ));
        }
        let concurrency = i64::try_from(limits.max_concurrency).map_err(|_| unavailable())?;
        let rpm = i64::try_from(limits.requests_per_minute).map_err(|_| unavailable())?;
        let mut transaction = self.pool.begin().await.map_err(|_| unavailable())?;
        let revision = bump_config_revision_in_transaction(&mut transaction)
            .await
            .map_err(|error| admin_store_error("global admission", error))?;
        sqlx::query("update runtime_settings set global_max_concurrency = $1, global_requests_per_minute = $2, updated_at = now() where id = 1")
            .bind(concurrency).bind(rpm).execute(&mut *transaction).await.map_err(|_| unavailable())?;
        let audit = mutation_audit(
            context,
            "settings.admission",
            "runtime_settings",
            "1",
            vec![
                "global_max_concurrency".to_owned(),
                "global_requests_per_minute".to_owned(),
            ],
        );
        append_admin_audit_event_in_transaction(&mut transaction, audit, revision)
            .await
            .map_err(|error| admin_store_error("global admission", error))?;
        transaction.commit().await.map_err(|_| unavailable())?;
        Ok(GlobalAdmissionSettings {
            limits,
            config_revision: admin_revision(revision)?,
        })
    }
}

fn unavailable() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "global admission",
        "全站限额配置暂不可用",
    )
}
