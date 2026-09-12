//! 共享配额归属和共享限额的管理用例。

use async_trait::async_trait;
use gateway_core::{identity::QuotaScopeId, runtime::SnapshotControl};
use std::sync::Arc;
use uuid::Uuid;

use super::{map_store_error, publish_committed};
use crate::{
    model::{
        AdminError, MutationContext,
        quota_scopes::{
            QuotaScopeChange, QuotaScopeFields, QuotaScopeListQuery, QuotaScopeMutation,
            QuotaScopePage,
        },
    },
    ports::store::QuotaScopeStore,
};

#[async_trait]
pub trait QuotaScopeService: Send + Sync {
    async fn list(&self, query: QuotaScopeListQuery) -> Result<QuotaScopePage, AdminError>;
    async fn create(
        &self,
        context: &MutationContext,
        fields: QuotaScopeFields,
    ) -> Result<QuotaScopeMutation, AdminError>;
    async fn change(
        &self,
        context: &MutationContext,
        command: QuotaScopeChange,
    ) -> Result<QuotaScopeMutation, AdminError>;
}

pub(crate) struct DefaultQuotaScopeService {
    store: Arc<dyn QuotaScopeStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultQuotaScopeService {
    pub(crate) fn new(store: Arc<dyn QuotaScopeStore>, snapshot: Arc<dyn SnapshotControl>) -> Self {
        Self { store, snapshot }
    }
}

#[async_trait]
impl QuotaScopeService for DefaultQuotaScopeService {
    async fn list(&self, query: QuotaScopeListQuery) -> Result<QuotaScopePage, AdminError> {
        query.validate()?;
        self.store
            .list_quota_scopes(query)
            .await
            .map_err(|error| map_store_error(error, "quota_scope"))
    }

    async fn create(
        &self,
        context: &MutationContext,
        fields: QuotaScopeFields,
    ) -> Result<QuotaScopeMutation, AdminError> {
        let id = QuotaScopeId::new(format!("quota_{}", Uuid::now_v7().simple()))
            .map_err(|_| AdminError::internal("创建共享配额 ID 失败"))?;
        self.change(context, QuotaScopeChange::Create { id, fields })
            .await
    }

    async fn change(
        &self,
        context: &MutationContext,
        command: QuotaScopeChange,
    ) -> Result<QuotaScopeMutation, AdminError> {
        command.validate()?;
        let id = command.entity_ref().to_owned();
        let config_revision = self
            .store
            .change_quota_scope(command, context)
            .await
            .map_err(|error| map_store_error(error, "quota_scope"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(QuotaScopeMutation {
            id,
            config_revision,
        })
    }
}
