use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use gateway_core::task::{
    ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind,
    WorkerLeaseRequest, WorkerRegistration, WorkerRunnable, WorkerSchedule, WorkerTaskError,
};

use crate::{AdminServices, model::AdminError, ports::store::ChannelStore};

pub struct ChannelDiscoveryTask {
    store: Arc<dyn ChannelStore>,
    services: AdminServices,
}

impl ChannelDiscoveryTask {
    #[must_use]
    pub fn new(store: Arc<dyn ChannelStore>, services: AdminServices) -> Self {
        Self { store, services }
    }

    pub fn contribution(self) -> Result<WorkerContribution, AdminError> {
        let invalid = |_| AdminError::internal("模型发现 Worker 定义不合法");
        let id = WorkerId::try_new(WorkerKind::ChannelModelDiscovery, "channel-discovery")
            .map_err(invalid)?;
        let schedule = WorkerSchedule::try_new(
            Duration::from_secs(30),
            Duration::from_secs(5),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(20),
        )
        .map_err(invalid)?;
        let lease = WorkerLeaseRequest::try_new(id.clone(), schedule.leader_lease_ttl())
            .map_err(invalid)?;
        let registration = WorkerRegistration::try_new(
            id,
            WorkerRunnable::Scheduled {
                schedule,
                lease: Some(lease),
                task: Box::new(self),
            },
        )
        .map_err(invalid)?;
        Ok(WorkerContribution::Registration(registration))
    }

    async fn execute(&self) -> Result<(), WorkerTaskError> {
        let claim = self
            .store
            .claim_due_model_discovery()
            .await
            .map_err(|_| WorkerTaskError::safe("模型发现计划读取失败"))?;
        let Some(claim) = claim else {
            return Ok(());
        };
        let succeeded = self
            .services
            .channels()
            .discover_models(&claim.id, claim.revision)
            .await
            .is_ok();
        self.store
            .finish_scheduled_model_discovery(&claim, succeeded)
            .await
            .map_err(|_| WorkerTaskError::safe("模型发现完成状态未确认，请刷新渠道与发现历史"))?;
        if !succeeded {
            return Err(WorkerTaskError::safe(
                "定时模型发现未确认成功，将在下个间隔重试",
            ));
        }
        Ok(())
    }
}

impl ScheduledTask for ChannelDiscoveryTask {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            tokio::select! {
                biased;
                () = context.cancellation().cancelled() => Ok(()),
                result = tokio::time::timeout(Duration::from_secs(30), self.execute()) => {
                    result.map_err(|_| WorkerTaskError::safe("定时模型发现超时，完成状态未确认"))?
                }
            }
        })
    }
}
