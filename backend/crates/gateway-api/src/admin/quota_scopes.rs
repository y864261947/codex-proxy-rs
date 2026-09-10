//! 共享上游配额的管理员 HTTP 接口。

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
    quota_scopes::{
        QuotaScopeChange, QuotaScopeFields, QuotaScopeListQuery, QuotaScopeMutation,
        QuotaScopeRecord,
    },
};
use gateway_core::{identity::QuotaScopeId, policy::RateLimits};
use serde::{Deserialize, Serialize};

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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuotaScopeRequest {
    id: Option<String>,
    name: String,
    note: Option<String>,
    enabled: bool,
    max_concurrency: u64,
    requests_per_minute: u64,
}

impl QuotaScopeRequest {
    fn fields(self) -> QuotaScopeFields {
        QuotaScopeFields {
            name: self.name,
            note: self.note,
            enabled: self.enabled,
            limits: RateLimits {
                max_concurrency: self.max_concurrency,
                requests_per_minute: self.requests_per_minute,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdRequest {
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaScopeView {
    id: String,
    name: String,
    note: Option<String>,
    enabled: bool,
    max_concurrency: u64,
    requests_per_minute: u64,
    source_count: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<QuotaScopeRecord> for QuotaScopeView {
    fn from(record: QuotaScopeRecord) -> Self {
        Self {
            id: record.id.to_string(),
            name: record.fields.name,
            note: record.fields.note,
            enabled: record.fields.enabled,
            max_concurrency: record.fields.limits.max_concurrency,
            requests_per_minute: record.fields.limits.requests_per_minute,
            source_count: record.source_count,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaScopePageView {
    items: Vec<QuotaScopeView>,
    total: u64,
    config_revision: u64,
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
        .route("/api/admin/quota-scopes", get(list::<S>))
        .route("/api/admin/quota-scopes/create", post(create::<S>))
        .route("/api/admin/quota-scopes/update", post(update::<S>))
        .route("/api/admin/quota-scopes/delete", post(delete::<S>))
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
        .quota_scopes()
        .list(QuotaScopeListQuery {
            page: query.page.unwrap_or(1),
            page_size: PageSize::new(query.page_size.unwrap_or(50))
                .map_err(|_| AdminError::bad_request("共享配额分页参数不合法"))?,
            search: query
                .search
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(QuotaScopePageView {
            items: page.items.into_iter().map(QuotaScopeView::from).collect(),
            total: page.total,
            config_revision: page.config_revision.get(),
        }),
    ))
}

async fn create<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<QuotaScopeRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.id.is_some() {
        return Err(AdminError::bad_request("创建共享配额不能指定 ID"));
    }
    mutation_response(
        StatusCode::CREATED,
        state
            .admin_services()
            .quota_scopes()
            .create(&auth.context().mutation_context(), request.fields())
            .await,
    )
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(mut request): AdminJson<QuotaScopeRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = quota_scope_id(
        request
            .id
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少共享配额 ID"))?,
    )?;
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .quota_scopes()
            .change(
                &auth.context().mutation_context(),
                QuotaScopeChange::Update {
                    id,
                    fields: request.fields(),
                },
            )
            .await,
    )
}

async fn delete<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<IdRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .quota_scopes()
            .change(
                &auth.context().mutation_context(),
                QuotaScopeChange::Delete {
                    id: quota_scope_id(request.id)?,
                },
            )
            .await,
    )
}

fn quota_scope_id(value: String) -> Result<QuotaScopeId, AdminError> {
    QuotaScopeId::new(value).map_err(|_| AdminError::bad_request("共享配额 ID 不合法"))
}
fn mutation_response(
    status: StatusCode,
    result: Result<QuotaScopeMutation, gateway_admin::model::AdminError>,
) -> Result<AdminResponse<AdminEnvelope<MutationView>>, AdminError> {
    let mutation = result.map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        status,
        AdminEnvelope::ok(MutationView {
            id: mutation.id,
            config_revision: mutation.config_revision.get(),
        }),
    ))
}
