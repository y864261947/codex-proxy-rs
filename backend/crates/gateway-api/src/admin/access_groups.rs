//! 轻量接入分组管理和 Key 归属的管理员 HTTP 接口。

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
    access_groups::{
        AccessGroupChange, AccessGroupFields, AccessGroupListQuery, AccessGroupMutation,
        AccessGroupRecord,
    },
};
use gateway_core::account::scope::AccountGroupId;
use gateway_core::policy::{AccessGroupId, ClientApiKeyId, RateLimits};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
struct AccessGroupRequest {
    allowed_models: BTreeSet<String>,
    pool_group_ids: Vec<String>,
    id: Option<String>,
    name: String,
    note: Option<String>,
    enabled: bool,
    max_concurrency: u64,
    requests_per_minute: u64,
}

impl AccessGroupRequest {
    fn fields(self) -> Result<AccessGroupFields, AdminError> {
        Ok(AccessGroupFields {
            allowed_models: self.allowed_models,
            pool_group_ids: self
                .pool_group_ids
                .into_iter()
                .map(|id| {
                    AccountGroupId::new(id).map_err(|_| AdminError::bad_request("号池 ID 不合法"))
                })
                .collect::<Result<_, _>>()?,
            name: self.name,
            note: self.note,
            enabled: self.enabled,
            limits: RateLimits {
                max_concurrency: self.max_concurrency,
                requests_per_minute: self.requests_per_minute,
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdRequest {
    id: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AccessGroupBinding {
    Assigned(String),
    Unassigned(()),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignRequest {
    key_id: String,
    access_group_id: AccessGroupBinding,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessGroupView {
    allowed_models: Vec<String>,
    pool_group_ids: Vec<String>,
    id: String,
    name: String,
    note: Option<String>,
    enabled: bool,
    max_concurrency: u64,
    requests_per_minute: u64,
    key_count: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<AccessGroupRecord> for AccessGroupView {
    fn from(record: AccessGroupRecord) -> Self {
        Self {
            allowed_models: record.fields.allowed_models.into_iter().collect(),
            pool_group_ids: record
                .fields
                .pool_group_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            id: record.id.to_string(),
            name: record.fields.name,
            note: record.fields.note,
            enabled: record.fields.enabled,
            max_concurrency: record.fields.limits.max_concurrency,
            requests_per_minute: record.fields.limits.requests_per_minute,
            key_count: record.key_count,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessGroupPageView {
    items: Vec<AccessGroupView>,
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
        .route("/api/admin/access-groups", get(list::<S>))
        .route("/api/admin/access-groups/create", post(create::<S>))
        .route("/api/admin/access-groups/update", post(update::<S>))
        .route("/api/admin/access-groups/delete", post(delete::<S>))
        .route("/api/admin/access-groups/assign-key", post(assign::<S>))
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
        .access_groups()
        .list(AccessGroupListQuery {
            page: query.page.unwrap_or(1),
            page_size: PageSize::new(query.page_size.unwrap_or(50))
                .map_err(|_| AdminError::bad_request("接入分组分页参数不合法"))?,
            search: query
                .search
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(AccessGroupPageView {
            items: page.items.into_iter().map(AccessGroupView::from).collect(),
            total: page.total,
            config_revision: page.config_revision.get(),
        }),
    ))
}

async fn create<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AccessGroupRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.id.is_some() {
        return Err(AdminError::bad_request("创建接入分组不能指定 ID"));
    }
    mutation_response(
        StatusCode::CREATED,
        state
            .admin_services()
            .access_groups()
            .create(&auth.context().mutation_context(), request.fields()?)
            .await,
    )
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(mut request): AdminJson<AccessGroupRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = access_group_id(
        request
            .id
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少接入分组 ID"))?,
    )?;
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .access_groups()
            .change(
                &auth.context().mutation_context(),
                AccessGroupChange::Update {
                    id,
                    fields: request.fields()?,
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
            .access_groups()
            .change(
                &auth.context().mutation_context(),
                AccessGroupChange::Delete {
                    id: access_group_id(request.id)?,
                },
            )
            .await,
    )
}

async fn assign<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AssignRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let access_group_id = match request.access_group_id {
        AccessGroupBinding::Assigned(id) => Some(access_group_id(id)?),
        AccessGroupBinding::Unassigned(()) => None,
    };
    let key_id = ClientApiKeyId::new(request.key_id)
        .map_err(|_| AdminError::bad_request("Key ID 不合法"))?;
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .access_groups()
            .change(
                &auth.context().mutation_context(),
                AccessGroupChange::AssignKey {
                    key_id,
                    access_group_id,
                },
            )
            .await,
    )
}

fn access_group_id(value: String) -> Result<AccessGroupId, AdminError> {
    AccessGroupId::new(value).map_err(|_| AdminError::bad_request("接入分组 ID 不合法"))
}
fn mutation_response(
    status: StatusCode,
    result: Result<AccessGroupMutation, gateway_admin::model::AdminError>,
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
