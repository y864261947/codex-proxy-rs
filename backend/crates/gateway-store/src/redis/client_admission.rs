//! Client API Key 的 Redis RPM/并发原子准入与热状态恢复。

use std::{collections::HashSet, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_core::engine::admission::{
    ClientAdmissionDecision as CoreAdmissionDecision, ClientAdmissionError as CoreAdmissionError,
    ClientAdmissionPort, ClientAdmissionRecovery as CoreAdmissionRecovery,
    ClientAdmissionRejection as CoreAdmissionRejection,
    ClientAdmissionRequest as CoreAdmissionRequest,
    ClientAdmissionRestoreResult as CoreAdmissionRestoreResult,
};
use redis::{Script, aio::ConnectionManager};

use crate::{StoreError, StoreResult, redis_unavailable, require_nonempty};

use super::{MAX_REDIS_EXACT_INTEGER, namespace, resource_fingerprint};

const ADMIT_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now_ms = (tonumber(clock[1]) * 1000) + math.floor(tonumber(clock[2]) / 1000)
local cutoff = now_ms - 60000
local lease_ttl_ms = tonumber(ARGV[2])
if now_ms + lease_ttl_ms > tonumber(ARGV[3]) then return 3 end

-- Check every scope before reserving any of them. Replayed admissions use the
-- same request ID and must not consume a second slot or move the RPM timestamp.
for index = 1, #KEYS, 2 do
  redis.call('ZREMRANGEBYSCORE', KEYS[index], '-inf', now_ms)
  redis.call('ZREMRANGEBYSCORE', KEYS[index + 1], '-inf', cutoff)
  local concurrency = tonumber(ARGV[index + 3])
  local rpm = tonumber(ARGV[index + 4])
  if concurrency > 0 and not redis.call('ZSCORE', KEYS[index], ARGV[1])
      and redis.call('ZCARD', KEYS[index]) >= concurrency then return 2 end
  if rpm > 0 and not redis.call('ZSCORE', KEYS[index + 1], ARGV[1])
      and redis.call('ZCARD', KEYS[index + 1]) >= rpm then return 1 end
end

local function extend_ttl(key, ttl)
  local current = redis.call('PTTL', key)
  if current < ttl then redis.call('PEXPIRE', key, ttl) end
end

for index = 1, #KEYS, 2 do
  redis.call('ZADD', KEYS[index], 'NX', now_ms + lease_ttl_ms, ARGV[1])
  redis.call('ZADD', KEYS[index + 1], 'NX', now_ms, ARGV[1])
  local active_tail = redis.call('ZRANGE', KEYS[index], -1, -1, 'WITHSCORES')
  local active_ttl = 120000
  if #active_tail == 2 then
    active_ttl = math.max(active_ttl, tonumber(active_tail[2]) - now_ms + 60000)
  end
  extend_ttl(KEYS[index], active_ttl)
  extend_ttl(KEYS[index + 1], 120000)
end
return 0
"#;

const RELEASE_SCRIPT: &str = r#"
local removed = 0
for _, key in ipairs(KEYS) do
  removed = removed + redis.call('ZREM', key, ARGV[1])
end
return removed
"#;

const RESTORE_SCRIPT: &str = r#"
local clock = redis.call('TIME')
local now_ms = (tonumber(clock[1]) * 1000) + math.floor(tonumber(clock[2]) / 1000)
local cutoff = now_ms - 60000
local recent_count = tonumber(ARGV[1])
local cursor = 2

for _ = 1, recent_count do
  local started_at_ms = tonumber(ARGV[cursor + 1])
  if started_at_ms > now_ms then return {-1, 0, 0} end
  cursor = cursor + 2
end

local running_count = tonumber(ARGV[cursor])
local running_cursor = cursor + 1

redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now_ms)
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', cutoff)

local restored_recent = 0
cursor = 2
for _ = 1, recent_count do
  local request_id = ARGV[cursor]
  local started_at_ms = tonumber(ARGV[cursor + 1])
  if started_at_ms > cutoff then
    local inserted = redis.call('ZADD', KEYS[2], 'NX', started_at_ms, request_id)
    if inserted == 1 then
      restored_recent = restored_recent + 1
    end
  end
  cursor = cursor + 2
end

local restored_running = 0
for _ = 1, running_count do
  local request_id = ARGV[running_cursor]
  local expires_at_ms = tonumber(ARGV[running_cursor + 1])
  if expires_at_ms > now_ms then
    restored_running = restored_running
      + redis.call('ZADD', KEYS[1], 'NX', expires_at_ms, request_id)
  end
  running_cursor = running_cursor + 2
end

local function extend_ttl(key, ttl)
  if redis.call('ZCARD', key) == 0 then return end
  local current = redis.call('PTTL', key)
  if current < ttl then redis.call('PEXPIRE', key, ttl) end
end

local active_tail = redis.call('ZRANGE', KEYS[1], -1, -1, 'WITHSCORES')
local active_ttl = 120000
if #active_tail == 2 then
  active_ttl = math.max(active_ttl, tonumber(active_tail[2]) - now_ms + 60000)
end
extend_ttl(KEYS[1], active_ttl)
extend_ttl(KEYS[2], 120000)
return {0, restored_recent, restored_running}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAdmissionLimits {
    pub max_concurrency: u64,
    pub requests_per_minute: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionScope {
    pub scope_ref: String,
    pub limits: ClientAdmissionLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRequest {
    pub model_request_id: String,
    pub lease_ttl: Duration,
    pub scopes: Vec<ClientAdmissionScope>,
}

impl ClientAdmissionRequest {
    pub fn validate(&self) -> StoreResult<()> {
        require_nonempty(
            "client admission",
            "model_request_id",
            &self.model_request_id,
        )?;
        if self.scopes.is_empty() || self.scopes.len() > 32 {
            return Err(invalid("admission requires between 1 and 32 scopes"));
        }
        let mut scopes = HashSet::new();
        for scope in &self.scopes {
            require_nonempty("client admission", "scope_ref", &scope.scope_ref)?;
            if !scopes.insert(&scope.scope_ref) {
                return Err(invalid("admission scopes must be unique"));
            }
            redis_integer(scope.limits.max_concurrency, "maximum concurrency")?;
            redis_integer(scope.limits.requests_per_minute, "requests per minute")?;
        }
        if self.lease_ttl.as_millis() == 0 {
            return Err(invalid("lease TTL must be positive"));
        }
        redis_duration_millis(self.lease_ttl)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRecentRequest {
    pub model_request_id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRunningRequest {
    pub model_request_id: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAdmissionRestore {
    pub scope_ref: String,
    pub recent_requests: Vec<ClientAdmissionRecentRequest>,
    pub running_requests: Vec<ClientAdmissionRunningRequest>,
}

impl ClientAdmissionRestore {
    pub fn validate(&self) -> StoreResult<()> {
        require_nonempty("client admission", "scope_ref", &self.scope_ref)?;
        redis_len(self.recent_requests.len(), "recent request count")?;
        redis_len(self.running_requests.len(), "running request count")?;

        let mut recent_ids = HashSet::with_capacity(self.recent_requests.len());
        for request in &self.recent_requests {
            validate_recovery_request_id(&request.model_request_id)?;
            redis_timestamp_millis(request.started_at, "request start time")?;
            if !recent_ids.insert(request.model_request_id.as_str()) {
                return Err(invalid("recent request IDs must be unique"));
            }
        }

        let mut running_ids = HashSet::with_capacity(self.running_requests.len());
        for request in &self.running_requests {
            validate_recovery_request_id(&request.model_request_id)?;
            redis_timestamp_millis(request.expires_at, "request expiry time")?;
            if !running_ids.insert(request.model_request_id.as_str()) {
                return Err(invalid("running request IDs must be unique"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAdmissionRestoreResult {
    pub restored_recent_requests: u64,
    pub restored_running_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAdmissionRejection {
    RateLimited,
    ConcurrencyLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAdmissionDecision {
    Granted,
    Rejected(ClientAdmissionRejection),
}

#[async_trait]
pub trait ClientAdmissionRepository: Send + Sync {
    async fn admit_client_request(
        &self,
        request: &ClientAdmissionRequest,
    ) -> StoreResult<ClientAdmissionDecision>;
    async fn release_client_request(
        &self,
        scope_refs: &[String],
        model_request_id: &str,
    ) -> StoreResult<bool>;
    async fn restore_client_admission(
        &self,
        recovery: &ClientAdmissionRestore,
    ) -> StoreResult<ClientAdmissionRestoreResult>;
    async fn clear_client_admission(&self, scope_ref: &str) -> StoreResult<()>;
}

#[derive(Clone)]
pub struct RedisClientAdmissionRepository {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisClientAdmissionRepository {
    pub fn new(connection: ConnectionManager, key_namespace: &str) -> StoreResult<Self> {
        Ok(Self {
            connection,
            namespace: namespace(key_namespace)?,
        })
    }

    fn keys(&self, scope_ref: &str) -> StoreResult<[String; 2]> {
        let fingerprint = resource_fingerprint("client admission", scope_ref)?;
        Ok([
            format!(
                "{}:client:{{admission}}:{fingerprint}:active",
                self.namespace
            ),
            format!(
                "{}:client:{{admission}}:{fingerprint}:requests",
                self.namespace
            ),
        ])
    }
}

#[async_trait]
impl ClientAdmissionRepository for RedisClientAdmissionRepository {
    async fn admit_client_request(
        &self,
        request: &ClientAdmissionRequest,
    ) -> StoreResult<ClientAdmissionDecision> {
        request.validate()?;
        let lease_ttl_ms = u64::try_from(request.lease_ttl.as_millis())
            .map_err(|_| invalid("lease TTL is too large"))?;
        let mut connection = self.connection.clone();
        let script = Script::new(ADMIT_SCRIPT);
        let mut invocation = script.prepare_invoke();
        invocation
            .arg(&request.model_request_id)
            .arg(lease_ttl_ms)
            .arg(MAX_REDIS_EXACT_INTEGER);
        for scope in &request.scopes {
            let keys = self.keys(&scope.scope_ref)?;
            invocation
                .key(&keys[0])
                .key(&keys[1])
                .arg(scope.limits.max_concurrency)
                .arg(scope.limits.requests_per_minute);
        }
        let code = invocation
            .invoke_async::<i64>(&mut connection)
            .await
            .map_err(|_| redis_unavailable("admit client request"))?;
        match code {
            0 => Ok(ClientAdmissionDecision::Granted),
            1 => Ok(ClientAdmissionDecision::Rejected(
                ClientAdmissionRejection::RateLimited,
            )),
            2 => Ok(ClientAdmissionDecision::Rejected(
                ClientAdmissionRejection::ConcurrencyLimited,
            )),
            3 => Err(invalid("lease expiry is outside the supported range")),
            _ => Err(invalid("Redis returned an unknown admission decision")),
        }
    }

    async fn release_client_request(
        &self,
        scope_refs: &[String],
        model_request_id: &str,
    ) -> StoreResult<bool> {
        require_nonempty("client admission", "model_request_id", model_request_id)?;
        if scope_refs.is_empty() || scope_refs.len() > 32 {
            return Err(invalid("release requires between 1 and 32 scopes"));
        }
        let keys = scope_refs
            .iter()
            .map(|scope| self.keys(scope).map(|keys| keys[0].clone()))
            .collect::<StoreResult<Vec<_>>>()?;
        let mut connection = self.connection.clone();
        let script = Script::new(RELEASE_SCRIPT);
        let mut invocation = script.prepare_invoke();
        for key in keys {
            invocation.key(key);
        }
        let removed = invocation
            .arg(model_request_id)
            .invoke_async::<i64>(&mut connection)
            .await
            .map_err(|_| redis_unavailable("release client request"))?;
        Ok(removed > 0)
    }

    async fn restore_client_admission(
        &self,
        recovery: &ClientAdmissionRestore,
    ) -> StoreResult<ClientAdmissionRestoreResult> {
        recovery.validate()?;
        let keys = self.keys(&recovery.scope_ref)?;
        let script = Script::new(RESTORE_SCRIPT);
        let mut invocation = script.prepare_invoke();
        invocation.key(&keys[0]).key(&keys[1]).arg(redis_len(
            recovery.recent_requests.len(),
            "recent request count",
        )?);
        for request in &recovery.recent_requests {
            invocation
                .arg(&request.model_request_id)
                .arg(redis_timestamp_millis(
                    request.started_at,
                    "request start time",
                )?);
        }
        invocation.arg(redis_len(
            recovery.running_requests.len(),
            "running request count",
        )?);
        for request in &recovery.running_requests {
            invocation
                .arg(&request.model_request_id)
                .arg(redis_timestamp_millis(
                    request.expires_at,
                    "request expiry time",
                )?);
        }

        let mut connection = self.connection.clone();
        let (code, restored_recent_requests, restored_running_requests) = invocation
            .invoke_async::<(i64, u64, u64)>(&mut connection)
            .await
            .map_err(|_| redis_unavailable("restore client admission"))?;
        if code == -1 {
            return Err(invalid("request start time is after Redis server time"));
        }
        if code != 0 {
            return Err(invalid("Redis returned an unknown recovery decision"));
        }
        Ok(ClientAdmissionRestoreResult {
            restored_recent_requests,
            restored_running_requests,
        })
    }

    async fn clear_client_admission(&self, client_api_key_ref: &str) -> StoreResult<()> {
        let keys = self.keys(client_api_key_ref)?;
        let mut connection = self.connection.clone();
        redis::cmd("DEL")
            .arg(&keys)
            .query_async::<i64>(&mut connection)
            .await
            .map_err(|_| redis_unavailable("clear client admission"))?;
        Ok(())
    }
}

impl ClientAdmissionPort for RedisClientAdmissionRepository {
    fn admit(
        &self,
        request: CoreAdmissionRequest,
    ) -> futures::future::BoxFuture<'_, Result<CoreAdmissionDecision, CoreAdmissionError>> {
        Box::pin(async move {
            self.admit_client_request(&ClientAdmissionRequest {
                model_request_id: request.model_request_id.as_str().to_owned(),
                lease_ttl: request.lease_ttl,
                scopes: request
                    .scopes
                    .into_iter()
                    .map(|scope| ClientAdmissionScope {
                        scope_ref: scope.id.to_string(),
                        limits: ClientAdmissionLimits {
                            max_concurrency: scope.limits.max_concurrency,
                            requests_per_minute: scope.limits.requests_per_minute,
                        },
                    })
                    .collect(),
            })
            .await
            .map(|decision| match decision {
                ClientAdmissionDecision::Granted => CoreAdmissionDecision::Granted,
                ClientAdmissionDecision::Rejected(ClientAdmissionRejection::RateLimited) => {
                    CoreAdmissionDecision::Rejected(CoreAdmissionRejection::RateLimited)
                }
                ClientAdmissionDecision::Rejected(ClientAdmissionRejection::ConcurrencyLimited) => {
                    CoreAdmissionDecision::Rejected(CoreAdmissionRejection::ConcurrencyLimited)
                }
            })
            .map_err(|_| CoreAdmissionError)
        })
    }

    fn release<'a>(
        &'a self,
        scope_ids: &'a [gateway_core::policy::AdmissionScopeId],
        model_request_id: &'a gateway_core::engine::ModelRequestId,
    ) -> futures::future::BoxFuture<'a, Result<bool, CoreAdmissionError>> {
        Box::pin(async move {
            let scope_refs = scope_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            self.release_client_request(&scope_refs, model_request_id.as_str())
                .await
                .map_err(|_| CoreAdmissionError)
        })
    }

    fn restore(
        &self,
        recovery: CoreAdmissionRecovery,
    ) -> futures::future::BoxFuture<'_, Result<CoreAdmissionRestoreResult, CoreAdmissionError>>
    {
        Box::pin(async move {
            self.restore_client_admission(&ClientAdmissionRestore {
                scope_ref: recovery.scope_id.to_string(),
                recent_requests: recovery
                    .recent_requests
                    .into_iter()
                    .map(|request| ClientAdmissionRecentRequest {
                        model_request_id: request.model_request_id.as_str().to_owned(),
                        started_at: DateTime::<Utc>::from(request.started_at),
                    })
                    .collect(),
                running_requests: recovery
                    .running_requests
                    .into_iter()
                    .map(|request| ClientAdmissionRunningRequest {
                        model_request_id: request.model_request_id.as_str().to_owned(),
                        expires_at: DateTime::<Utc>::from(request.expires_at),
                    })
                    .collect(),
            })
            .await
            .map(|restored| CoreAdmissionRestoreResult {
                restored_recent_requests: restored.restored_recent_requests,
                restored_running_requests: restored.restored_running_requests,
            })
            .map_err(|_| CoreAdmissionError)
        })
    }
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "client admission",
        message: message.to_owned(),
    }
}

fn validate_recovery_request_id(model_request_id: &str) -> StoreResult<()> {
    require_nonempty("client admission", "model_request_id", model_request_id)
}

fn redis_duration_millis(duration: Duration) -> StoreResult<u64> {
    let milliseconds =
        u64::try_from(duration.as_millis()).map_err(|_| invalid("lease TTL is too large"))?;
    redis_integer(milliseconds, "lease TTL")
}

fn redis_timestamp_millis(timestamp: DateTime<Utc>, field: &str) -> StoreResult<u64> {
    let milliseconds = u64::try_from(timestamp.timestamp_millis()).map_err(|_| invalid(field))?;
    redis_integer(milliseconds, field)
}

fn redis_len(length: usize, field: &str) -> StoreResult<u64> {
    let value = u64::try_from(length).map_err(|_| invalid(field))?;
    redis_integer(value, field)
}

fn redis_integer(value: u64, field: &str) -> StoreResult<u64> {
    if value > MAX_REDIS_EXACT_INTEGER {
        return Err(invalid(&format!(
            "{field} is outside Redis' exact integer range"
        )));
    }
    Ok(value)
}
