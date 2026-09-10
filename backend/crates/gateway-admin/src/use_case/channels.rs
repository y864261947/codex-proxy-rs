//! 渠道用例负责版本、事务提交和发布；连接字段的语义留在 Provider。

use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::{
    channel::ChannelRevision,
    identity::{ChannelId, ProviderKind},
    runtime::SnapshotControl,
};
use uuid::Uuid;

use super::{map_provider_error, map_store_error, publish_committed};
use crate::{
    model::{
        AdminError, MutationContext,
        channels::{
            ChannelChange, ChannelConnection, ChannelListQuery, ChannelMutation, ChannelPage,
            NewChannel, UpdateChannel,
        },
    },
    ports::{channels::ChannelAdminRegistry, store::ChannelStore},
};

#[async_trait]
pub trait ChannelService: Send + Sync {
    fn providers(&self) -> Vec<ProviderKind>;
    async fn list(&self, query: ChannelListQuery) -> Result<ChannelPage, AdminError>;
    async fn connection(&self, id: &ChannelId) -> Result<ChannelConnection, AdminError>;
    async fn create(
        &self,
        context: &MutationContext,
        command: NewChannel,
    ) -> Result<ChannelMutation, AdminError>;
    async fn update(
        &self,
        context: &MutationContext,
        command: UpdateChannel,
    ) -> Result<ChannelMutation, AdminError>;
    async fn delete(
        &self,
        context: &MutationContext,
        id: ChannelId,
        expected_revision: ChannelRevision,
    ) -> Result<ChannelMutation, AdminError>;
}

pub(crate) struct DefaultChannelService {
    store: Arc<dyn ChannelStore>,
    providers: ChannelAdminRegistry,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultChannelService {
    pub(crate) fn new(
        store: Arc<dyn ChannelStore>,
        providers: ChannelAdminRegistry,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            store,
            providers,
            snapshot,
        }
    }

    async fn commit(
        &self,
        context: &MutationContext,
        change: ChannelChange,
    ) -> Result<ChannelMutation, AdminError> {
        change.validate()?;
        let id = change.id().clone();
        let config_revision = self
            .store
            .change_channel(change, context)
            .await
            .map_err(|e| map_store_error(e, "channel"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(ChannelMutation {
            id,
            config_revision,
        })
    }
}

#[async_trait]
impl ChannelService for DefaultChannelService {
    fn providers(&self) -> Vec<ProviderKind> {
        self.providers.kinds().cloned().collect()
    }

    async fn list(&self, query: ChannelListQuery) -> Result<ChannelPage, AdminError> {
        query.validate()?;
        self.store
            .list_channels(query)
            .await
            .map_err(|e| map_store_error(e, "channel"))
    }

    async fn connection(&self, id: &ChannelId) -> Result<ChannelConnection, AdminError> {
        let stored = self
            .store
            .load_channel_for_edit(id)
            .await
            .map_err(|e| map_store_error(e, "channel"))?
            .ok_or_else(|| AdminError::not_found("渠道不存在"))?;
        let provider = self
            .providers
            .require(&stored.provider)
            .map_err(|e| map_provider_error(e, "channel"))?;
        let config = provider
            .public_config(&stored.config)
            .map_err(|e| map_provider_error(e, "channel"))?;
        Ok(ChannelConnection {
            id: stored.id,
            provider: stored.provider,
            revision: stored.revision,
            config,
        })
    }

    async fn create(
        &self,
        context: &MutationContext,
        command: NewChannel,
    ) -> Result<ChannelMutation, AdminError> {
        let id = ChannelId::new(format!("chan_{}", Uuid::now_v7().simple()))
            .map_err(|_| AdminError::internal("创建渠道 ID 失败"))?;
        command.fields.validate(&id)?;
        let provider = self
            .providers
            .require(&command.provider)
            .map_err(|e| map_provider_error(e, "channel"))?;
        let config = provider
            .prepare_config(&command.config, None)
            .map_err(|e| map_provider_error(e, "channel"))?;
        self.commit(
            context,
            ChannelChange::Create {
                id,
                provider: command.provider,
                fields: command.fields,
                config,
            },
        )
        .await
    }

    async fn update(
        &self,
        context: &MutationContext,
        command: UpdateChannel,
    ) -> Result<ChannelMutation, AdminError> {
        command.fields.validate(&command.id)?;
        let stored = self
            .store
            .load_channel_for_edit(&command.id)
            .await
            .map_err(|e| map_store_error(e, "channel"))?
            .ok_or_else(|| AdminError::not_found("渠道不存在"))?;
        if stored.revision != command.expected_revision {
            return Err(AdminError::conflict("渠道已更新，请刷新后重试"));
        }
        let replacement_config = command
            .config
            .as_ref()
            .map(|input| {
                let provider = self
                    .providers
                    .require(&stored.provider)
                    .map_err(|e| map_provider_error(e, "channel"))?;
                provider
                    .prepare_config(input, Some(&stored.config))
                    .map_err(|e| map_provider_error(e, "channel"))
            })
            .transpose()?;
        self.commit(
            context,
            ChannelChange::Update {
                id: command.id,
                expected_revision: command.expected_revision,
                fields: command.fields,
                replacement_config,
            },
        )
        .await
    }

    async fn delete(
        &self,
        context: &MutationContext,
        id: ChannelId,
        expected_revision: ChannelRevision,
    ) -> Result<ChannelMutation, AdminError> {
        self.commit(
            context,
            ChannelChange::Delete {
                id,
                expected_revision,
            },
        )
        .await
    }
}
