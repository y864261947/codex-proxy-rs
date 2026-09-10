//! 客户归属和共享限额的管理用例。

use async_trait::async_trait;
use gateway_core::{policy::CustomerId, runtime::SnapshotControl};
use std::sync::Arc;
use uuid::Uuid;

use super::{map_store_error, publish_committed};
use crate::{
    model::{
        AdminError, MutationContext,
        customers::{
            CustomerChange, CustomerFields, CustomerListQuery, CustomerMutation, CustomerPage,
        },
    },
    ports::store::CustomerStore,
};

#[async_trait]
pub trait CustomerService: Send + Sync {
    async fn list(&self, query: CustomerListQuery) -> Result<CustomerPage, AdminError>;
    async fn create(
        &self,
        context: &MutationContext,
        fields: CustomerFields,
    ) -> Result<CustomerMutation, AdminError>;
    async fn change(
        &self,
        context: &MutationContext,
        command: CustomerChange,
    ) -> Result<CustomerMutation, AdminError>;
}

pub(crate) struct DefaultCustomerService {
    store: Arc<dyn CustomerStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultCustomerService {
    pub(crate) fn new(store: Arc<dyn CustomerStore>, snapshot: Arc<dyn SnapshotControl>) -> Self {
        Self { store, snapshot }
    }
}

#[async_trait]
impl CustomerService for DefaultCustomerService {
    async fn list(&self, query: CustomerListQuery) -> Result<CustomerPage, AdminError> {
        query.validate()?;
        self.store
            .list_customers(query)
            .await
            .map_err(|error| map_store_error(error, "customer"))
    }

    async fn create(
        &self,
        context: &MutationContext,
        fields: CustomerFields,
    ) -> Result<CustomerMutation, AdminError> {
        let id = CustomerId::new(format!("cust_{}", Uuid::now_v7().simple()))
            .map_err(|_| AdminError::internal("创建客户 ID 失败"))?;
        self.change(context, CustomerChange::Create { id, fields })
            .await
    }

    async fn change(
        &self,
        context: &MutationContext,
        command: CustomerChange,
    ) -> Result<CustomerMutation, AdminError> {
        command.validate()?;
        let id = command.entity_ref().to_owned();
        let config_revision = self
            .store
            .change_customer(command, context)
            .await
            .map_err(|error| map_store_error(error, "customer"))?;
        publish_committed(self.snapshot.as_ref(), config_revision).await?;
        Ok(CustomerMutation {
            id,
            config_revision,
        })
    }
}
