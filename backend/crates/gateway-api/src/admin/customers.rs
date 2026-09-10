//! 轻量客户管理和 Key 归属的管理员 HTTP 接口。

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
    customers::{
        CustomerChange, CustomerFields, CustomerListQuery, CustomerMutation, CustomerRecord,
    },
};
use gateway_core::policy::{ClientApiKeyId, CustomerId, RateLimits};
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
struct CustomerRequest {
    id: Option<String>,
    name: String,
    note: Option<String>,
    enabled: bool,
    max_concurrency: u64,
    requests_per_minute: u64,
}

impl CustomerRequest {
    fn fields(self) -> CustomerFields {
        CustomerFields {
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

#[derive(Deserialize)]
#[serde(untagged)]
enum CustomerBinding {
    Assigned(String),
    Unassigned(()),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignRequest {
    key_id: String,
    customer_id: CustomerBinding,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomerView {
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

impl From<CustomerRecord> for CustomerView {
    fn from(record: CustomerRecord) -> Self {
        Self {
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
struct CustomerPageView {
    items: Vec<CustomerView>,
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
        .route("/api/admin/customers", get(list::<S>))
        .route("/api/admin/customers/create", post(create::<S>))
        .route("/api/admin/customers/update", post(update::<S>))
        .route("/api/admin/customers/delete", post(delete::<S>))
        .route("/api/admin/customers/assign-key", post(assign::<S>))
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
        .customers()
        .list(CustomerListQuery {
            page: query.page.unwrap_or(1),
            page_size: PageSize::new(query.page_size.unwrap_or(50))
                .map_err(|_| AdminError::bad_request("客户分页参数不合法"))?,
            search: query
                .search
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(CustomerPageView {
            items: page.items.into_iter().map(CustomerView::from).collect(),
            total: page.total,
            config_revision: page.config_revision.get(),
        }),
    ))
}

async fn create<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<CustomerRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    if request.id.is_some() {
        return Err(AdminError::bad_request("创建客户不能指定 ID"));
    }
    mutation_response(
        StatusCode::CREATED,
        state
            .admin_services()
            .customers()
            .create(&auth.context().mutation_context(), request.fields())
            .await,
    )
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(mut request): AdminJson<CustomerRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let id = customer_id(
        request
            .id
            .take()
            .ok_or_else(|| AdminError::bad_request("缺少客户 ID"))?,
    )?;
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .customers()
            .change(
                &auth.context().mutation_context(),
                CustomerChange::Update {
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
            .customers()
            .change(
                &auth.context().mutation_context(),
                CustomerChange::Delete {
                    id: customer_id(request.id)?,
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
    let customer_id = match request.customer_id {
        CustomerBinding::Assigned(id) => Some(customer_id(id)?),
        CustomerBinding::Unassigned(()) => None,
    };
    let key_id = ClientApiKeyId::new(request.key_id)
        .map_err(|_| AdminError::bad_request("Key ID 不合法"))?;
    mutation_response(
        StatusCode::OK,
        state
            .admin_services()
            .customers()
            .change(
                &auth.context().mutation_context(),
                CustomerChange::AssignKey {
                    key_id,
                    customer_id,
                },
            )
            .await,
    )
}

fn customer_id(value: String) -> Result<CustomerId, AdminError> {
    CustomerId::new(value).map_err(|_| AdminError::bad_request("客户 ID 不合法"))
}
fn mutation_response(
    status: StatusCode,
    result: Result<CustomerMutation, gateway_admin::model::AdminError>,
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
