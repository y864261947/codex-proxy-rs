//! Redis 丢失后从 `model_requests` 恢复客户端准入热状态。

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_core::{
    engine::{
        ModelRequestId,
        admission::{
            ClientAdmissionError, ClientAdmissionRecovery as CoreAdmissionRecovery,
            ClientAdmissionRecoveryPort, RecentAdmissionFact, RunningAdmissionFact,
        },
    },
    policy::{AdmissionScopeId, ClientApiKeyId, CustomerId},
};
use sqlx::PgPool;

use crate::{StoreResult, postgres_unavailable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRecentRequest {
    pub model_request_id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRunningRequest {
    pub model_request_id: String,
    pub deadline_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRecovery {
    pub scope_id: AdmissionScopeId,
    pub recent_requests: Vec<ClientAdmissionRecentRequest>,
    pub running_requests: Vec<ClientAdmissionRunningRequest>,
}

#[async_trait]
pub trait ClientAdmissionRecoveryRepository: Send + Sync {
    async fn load_client_admission_recovery(
        &self,
        window_started_at: DateTime<Utc>,
    ) -> StoreResult<Vec<ClientAdmissionRecovery>>;
}

#[derive(Clone)]
pub struct PgClientAdmissionRecoveryRepository {
    pool: PgPool,
}

impl PgClientAdmissionRecoveryRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ClientAdmissionRecoveryRepository for PgClientAdmissionRecoveryRepository {
    async fn load_client_admission_recovery(
        &self,
        window_started_at: DateTime<Utc>,
    ) -> StoreResult<Vec<ClientAdmissionRecovery>> {
        let rows = sqlx::query_as::<_, (String, String, String, DateTime<Utc>, DateTime<Utc>, String)>(
            "select scope.kind, scope.ref, r.id, r.started_at, r.deadline_at, r.outcome
             from model_requests r
             cross join lateral (values ('key', r.client_api_key_ref), ('customer', r.customer_ref)) scope(kind, ref)
             where scope.ref is not null and (r.started_at >= $1 or r.outcome = 'running')
             order by scope.kind, scope.ref, r.started_at, r.id",
        )
        .bind(window_started_at)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| postgres_unavailable("load client admission recovery"))?;
        let mut recoveries = BTreeMap::<AdmissionScopeId, ClientAdmissionRecovery>::new();
        for (kind, reference, model_request_id, started_at, deadline_at, outcome) in rows {
            let scope_id = match kind.as_str() {
                "key" => AdmissionScopeId::Key(
                    ClientApiKeyId::new(reference)
                        .map_err(|_| postgres_unavailable("invalid admission key ref"))?,
                ),
                "customer" => AdmissionScopeId::Customer(
                    CustomerId::new(reference)
                        .map_err(|_| postgres_unavailable("invalid admission customer ref"))?,
                ),
                _ => return Err(postgres_unavailable("unknown admission scope kind")),
            };
            let recovery =
                recoveries
                    .entry(scope_id.clone())
                    .or_insert_with(|| ClientAdmissionRecovery {
                        scope_id,
                        recent_requests: Vec::new(),
                        running_requests: Vec::new(),
                    });
            if started_at >= window_started_at {
                recovery.recent_requests.push(ClientAdmissionRecentRequest {
                    model_request_id: model_request_id.clone(),
                    started_at,
                });
            }
            if outcome == "running" {
                recovery
                    .running_requests
                    .push(ClientAdmissionRunningRequest {
                        model_request_id,
                        deadline_at,
                    });
            }
        }
        Ok(recoveries.into_values().collect())
    }
}

impl ClientAdmissionRecoveryPort for PgClientAdmissionRecoveryRepository {
    fn load_recovery(
        &self,
        since: std::time::SystemTime,
    ) -> futures::future::BoxFuture<'_, Result<Vec<CoreAdmissionRecovery>, ClientAdmissionError>>
    {
        Box::pin(async move {
            self.load_client_admission_recovery(DateTime::<Utc>::from(since))
                .await
                .map_err(|_| ClientAdmissionError)?
                .into_iter()
                .map(|recovery| {
                    let recent_requests = recovery
                        .recent_requests
                        .into_iter()
                        .map(|request| {
                            Ok(RecentAdmissionFact {
                                model_request_id: ModelRequestId::new(request.model_request_id)
                                    .map_err(|_| ClientAdmissionError)?,
                                started_at: request.started_at.into(),
                            })
                        })
                        .collect::<Result<Vec<_>, ClientAdmissionError>>()?;
                    let running_requests = recovery
                        .running_requests
                        .into_iter()
                        .map(|request| {
                            Ok(RunningAdmissionFact {
                                model_request_id: ModelRequestId::new(request.model_request_id)
                                    .map_err(|_| ClientAdmissionError)?,
                                expires_at: request.deadline_at.into(),
                            })
                        })
                        .collect::<Result<Vec<_>, ClientAdmissionError>>()?;
                    Ok(CoreAdmissionRecovery {
                        scope_id: recovery.scope_id,
                        recent_requests,
                        running_requests,
                    })
                })
                .collect()
        })
    }
}
