//! 接入分组归属和共享限额的管理用例。

use async_trait::async_trait;
use gateway_core::{policy::AccessGroupId, runtime::SnapshotControl};
use std::sync::Arc;
use uuid::Uuid;

use super::{map_store_error, publish_committed};
use crate::{
    model::{
        AdminError, MutationContext,
        access_groups::{
            AccessGroupChange, AccessGroupFields, AccessGroupListQuery, AccessGroupMutation,
            AccessGroupPage,
        },
    },
    ports::store::AccessGroupStore,
};

#[async_trait]
pub trait AccessGroupService: Send + Sync {
    async fn list(&self, query: AccessGroupListQuery) -> Result<AccessGroupPage, AdminError>;
    async fn create(
        &self,
        context: &MutationContext,
        fields: AccessGroupFields,
    ) -> Result<AccessGroupMutation, AdminError>;
    async fn change(
        &self,
        context: &MutationContext,
        command: AccessGroupChange,
    ) -> Result<AccessGroupMutation, AdminError>;
}

pub(crate) struct DefaultAccessGroupService {
    store: Arc<dyn AccessGroupStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultAccessGroupService {
    pub(crate) fn new(
        store: Arc<dyn AccessGroupStore>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self { store, snapshot }
    }
}

#[async_trait]
impl AccessGroupService for DefaultAccessGroupService {
    async fn list(&self, query: AccessGroupListQuery) -> Result<AccessGroupPage, AdminError> {
        query.validate()?;
        self.store
            .list_access_groups(query)
            .await
            .map_err(|error| map_store_error(error, "access_group"))
    }

    async fn create(
        &self,
        context: &MutationContext,
        fields: AccessGroupFields,
    ) -> Result<AccessGroupMutation, AdminError> {
        let id = AccessGroupId::new(format!("access_{}", Uuid::now_v7().simple()))
            .map_err(|_| AdminError::internal("创建接入分组 ID 失败"))?;
        self.change(context, AccessGroupChange::Create { id, fields })
            .await
    }

    async fn change(
        &self,
        context: &MutationContext,
        command: AccessGroupChange,
    ) -> Result<AccessGroupMutation, AdminError> {
        command.validate()?;
        let id = command.entity_ref().to_owned();
        let config_revision = self
            .store
            .change_access_group(command, context)
            .await
            .map_err(|error| map_store_error(error, "access_group"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(AccessGroupMutation {
            id,
            config_revision,
        })
    }
}
