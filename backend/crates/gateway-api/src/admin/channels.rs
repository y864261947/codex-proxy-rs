//! 通用渠道 HTTP adapter；Provider 配置保持对象透传，不解释凭据。

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gateway_admin::model::{
    PageSize,
    channels::{
        ChannelFields, ChannelListQuery, ChannelMutation, ChannelRecord, NewChannel, UpdateChannel,
    },
    provider_credentials::ProviderDocument,
};
use gateway_core::{
    account::OpaqueProviderData,
    channel::ChannelRevision,
    identity::{ChannelId, ProviderKind, QuotaScopeId},
    policy::RateLimits,
    routing::source::SourcePreference,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u16>,
    search: Option<String>,
    provider: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChannelRequest {
    id: Option<String>,
    expected_revision: Option<String>,
    provider: Option<String>,
    name: String,
    note: Option<String>,
    enabled: bool,
    priority: u16,
    weight: u16,
    max_concurrency: u64,
    requests_per_minute: u64,
    quota_scope_id: Option<String>,
    config: Option<Map<String, Value>>,
}

impl ChannelRequest {
    fn fields(self) -> Result<ChannelFields, AdminError> {
        Ok(ChannelFields {
            name: self.name,
            note: self.note,
            enabled: self.enabled,
            preference: SourcePreference::new(self.priority, self.weight)
                .map_err(|_| AdminError::bad_request("优先级和权重必须大于零"))?,
            limits: RateLimits {
                max_concurrency: self.max_concurrency,
                requests_per_minute: self.requests_per_minute,
            },
            quota_scope_id: self
                .quota_scope_id
                .map(QuotaScopeId::new)
                .transpose()
                .map_err(|_| AdminError::bad_request("共享配额 ID 不合法"))?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdQuery {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteRequest {
    id: String,
    expected_revision: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelView {
    id: String,
    provider: String,
    name: String,
    note: Option<String>,
    enabled: bool,
    priority: u16,
    weight: u16,
    max_concurrency: u64,
    requests_per_minute: u64,
    quota_scope_id: Option<String>,
    connection_revision: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ChannelRecord> for ChannelView {
    fn from(row: ChannelRecord) -> Self {
        Self {
            id: row.id.to_string(),
            provider: row.provider.to_string(),
            name: row.fields.name,
            note: row.fields.note,
            enabled: row.fields.enabled,
            priority: row.fields.preference.priority(),
            weight: row.fields.preference.weight(),
            max_concurrency: row.fields.limits.max_concurrency,
            requests_per_minute: row.fields.limits.requests_per_minute,
            quota_scope_id: row.fields.quota_scope_id.map(|id| id.to_string()),
            connection_revision: row.connection_revision.get().to_string(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageView {
    items: Vec<ChannelView>,
    total: u64,
    config_revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionView {
    id: String,
    provider: String,
    connection_revision: String,
    config: Map<String, Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MutationView {
    id: String,
    config_revision: u64,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/channels", get(list::<S>))
        .route("/api/admin/channels/providers", get(providers::<S>))
        .route("/api/admin/channels/connection", get(connection::<S>))
        .route("/api/admin/channels/create", post(create::<S>))
        .route("/api/admin/channels/update", post(update::<S>))
        .route("/api/admin/channels/delete", post(delete::<S>))
}

async fn providers<S>(_auth: AdminAuth, State(state): State<S>) -> impl IntoResponse
where
    S: AdminSessionState + Send + Sync,
{
    AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(
            state
                .admin_services()
                .channels()
                .providers()
                .into_iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>(),
        ),
    )
}

async fn list<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<ListQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let page = state
        .admin_services()
        .channels()
        .list(ChannelListQuery {
            page: query.page.unwrap_or(1),
            page_size: PageSize::new(query.page_size.unwrap_or(50))
                .map_err(|_| AdminError::bad_request("分页参数不合法"))?,
            search: query
                .search
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            provider: query.provider.map(provider_kind).transpose()?,
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(PageView {
            items: page.items.into_iter().map(ChannelView::from).collect(),
            total: page.total,
            config_revision: page.config_revision.get(),
        }),
    ))
}

async fn connection<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<IdQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let record = state
        .admin_services()
        .channels()
        .connection(&channel_id(query.id)?)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(ConnectionView {
            id: record.id.to_string(),
            provider: record.provider.to_string(),
            connection_revision: record.revision.get().to_string(),
            config: record.config.into_provider_data().into_inner(),
        }),
    ))
}

async fn create<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(mut request): AdminJson<ChannelRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.id.is_some() || request.expected_revision.is_some() {
        return Err(AdminError::bad_request("创建渠道不能指定 ID 或版本"));
    }
    let provider = provider_kind(
        request
            .provider
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少协议适配器"))?,
    )?;
    let config = document(
        request
            .config
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少连接配置"))?,
    );
    mutation_response(
        StatusCode::CREATED,
        state
            .admin_services()
            .channels()
            .create(
                &auth.context().mutation_context(),
                NewChannel {
                    provider,
                    fields: request.fields()?,
                    config,
                },
            )
            .await,
    )
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(mut request): AdminJson<ChannelRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.provider.is_some() {
        return Err(AdminError::bad_request("已有渠道不能修改协议适配器"));
    }
    let id = channel_id(
        request
            .id
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少渠道 ID"))?,
    )?;
    let expected_revision = revision(
        request
            .expected_revision
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少配置版本"))?,
    )?;
    let config = request.config.take().map(document);
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .channels()
            .update(
                &auth.context().mutation_context(),
                UpdateChannel {
                    id,
                    expected_revision,
                    fields: request.fields()?,
                    config,
                },
            )
            .await,
    )
}

async fn delete<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<DeleteRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .channels()
            .delete(
                &auth.context().mutation_context(),
                channel_id(request.id)?,
                revision(request.expected_revision)?,
            )
            .await,
    )
}

fn channel_id(id: String) -> Result<ChannelId, AdminError> {
    ChannelId::new(id).map_err(|_| AdminError::bad_request("渠道 ID 不合法"))
}
fn provider_kind(value: String) -> Result<ProviderKind, AdminError> {
    ProviderKind::new(value).map_err(|_| AdminError::bad_request("协议适配器不合法"))
}
fn revision(value: String) -> Result<ChannelRevision, AdminError> {
    value
        .parse::<u64>()
        .ok()
        .and_then(|n| ChannelRevision::new(n).ok())
        .ok_or_else(|| AdminError::bad_request("配置版本不合法"))
}
fn document(value: Map<String, Value>) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(value))
}
fn mutation_response(
    status: StatusCode,
    result: Result<ChannelMutation, gateway_admin::model::AdminError>,
) -> Result<AdminResponse<AdminEnvelope<MutationView>>, AdminError> {
    let result = result.map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        status,
        AdminEnvelope::ok(MutationView {
            id: result.id.to_string(),
            config_revision: result.config_revision.get(),
        }),
    ))
}
