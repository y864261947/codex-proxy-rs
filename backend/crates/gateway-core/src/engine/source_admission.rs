//! 每次上游尝试的来源容量；与下游逻辑请求准入独立。

use std::time::SystemTime;

use futures::future::BoxFuture;

use crate::{identity::QuotaScopeId, policy::RateLimits, routing::source::SourceId};

use super::provider::ResourceLease;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAdmissionRequest {
    pub source: SourceId,
    pub limits: RateLimits,
    /// 已解析并冻结的共享配额，不能用来源自己的限额代替。
    pub shared_quota: Option<(QuotaScopeId, RateLimits)>,
    pub deadline: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceAdmissionError {
    #[error("source capacity is temporarily unavailable")]
    Capacity,
    #[error("source admission infrastructure is unavailable")]
    Unavailable,
}

/// 返回的租约持有到 Provider 流终态或丢弃；取消等待也必须收敛已授予的占用。
pub trait SourceAdmissionPort: Send + Sync {
    fn acquire(
        &self,
        request: SourceAdmissionRequest,
    ) -> BoxFuture<'_, Result<Box<dyn ResourceLease>, SourceAdmissionError>>;
}
