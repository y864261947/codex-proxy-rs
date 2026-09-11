use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use gateway_admin::model::{
    PageSize,
    catalog::{CatalogSourceKind, ModelCatalogQuery},
};
use gateway_core::{
    catalog::CatalogModel, identity::ProviderKind, operation::Feature, routing::SupportLevel,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminQuery, AdminResponse, AdminSessionState,
    wire::map_admin_service_error,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u16>,
    search: Option<String>,
    provider: Option<String>,
    source_kind: Option<SourceKind>,
    configuration_ready: Option<bool>,
}

#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    Channel,
    AccountPool,
    Unpooled,
    ProviderCatalog,
}

impl From<SourceKind> for CatalogSourceKind {
    fn from(kind: SourceKind) -> Self {
        match kind {
            SourceKind::Channel => Self::Channel,
            SourceKind::AccountPool => Self::AccountPool,
            SourceKind::Unpooled => Self::Unpooled,
            SourceKind::ProviderCatalog => Self::ProviderCatalog,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelView {
    identity_key: String,
    provider: String,
    upstream_model: String,
    public_names: Vec<String>,
    display_name: Option<String>,
    description: Option<String>,
    source: SourceView,
    configuration_ready: bool,
    operations: Vec<String>,
    features: BTreeMap<String, &'static str>,
    upstream_validates_features: bool,
    context_window_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    hidden: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceView {
    kind: SourceKind,
    id: Option<String>,
    name: Option<String>,
    connection_revision: Option<String>,
    priority: Option<u16>,
    weight: Option<u16>,
    max_concurrency: Option<u64>,
    requests_per_minute: Option<u64>,
    quota_scope_id: Option<String>,
}

impl From<CatalogModel> for ModelView {
    fn from(model: CatalogModel) -> Self {
        let kind = match CatalogSourceKind::of(&model) {
            CatalogSourceKind::Channel => SourceKind::Channel,
            CatalogSourceKind::AccountPool => SourceKind::AccountPool,
            CatalogSourceKind::Unpooled => SourceKind::Unpooled,
            CatalogSourceKind::ProviderCatalog => SourceKind::ProviderCatalog,
        };
        let source_id = model
            .key
            .source
            .as_ref()
            .map(|source| source.reference().to_owned());
        let identity_key = serde_json::to_string(&(
            model.key.provider.as_str(),
            kind,
            &source_id,
            model.key.upstream_model.as_str(),
        ))
        .expect("catalog identity contains only strings");
        let features = [
            ("tools", Feature::Tools),
            ("vision", Feature::Vision),
            ("reasoning", Feature::Reasoning),
            ("json_schema", Feature::JsonSchema),
            ("native_continuation", Feature::NativeContinuation),
        ]
        .into_iter()
        .map(|(name, feature)| {
            let support = match model
                .capabilities
                .features()
                .get(&feature)
                .copied()
                .unwrap_or(SupportLevel::Unknown)
            {
                SupportLevel::Native => "native",
                SupportLevel::Emulated => "emulated",
                SupportLevel::Unsupported => "unsupported",
                SupportLevel::Unknown => "unknown",
            };
            (name.to_owned(), support)
        })
        .collect();
        let controls = model.source_controls.as_ref();
        let presentation = model.presentation.as_ref();
        Self {
            identity_key,
            provider: model.key.provider.to_string(),
            upstream_model: model.key.upstream_model.to_string(),
            public_names: model.public_names,
            display_name: presentation
                .and_then(|value| value.display_name())
                .map(str::to_owned),
            description: presentation
                .and_then(|value| value.description())
                .map(str::to_owned),
            source: SourceView {
                kind,
                id: source_id,
                name: model
                    .source
                    .as_ref()
                    .and_then(|source| source.name())
                    .map(str::to_owned),
                connection_revision: model
                    .connection_revision
                    .map(|revision| revision.get().to_string()),
                priority: controls.map(|value| value.preference().priority()),
                weight: controls.map(|value| value.preference().weight()),
                max_concurrency: controls.map(|value| value.limits().max_concurrency),
                requests_per_minute: controls.map(|value| value.limits().requests_per_minute),
                quota_scope_id: controls
                    .and_then(|value| value.quota_scope_id())
                    .map(ToString::to_string),
            },
            configuration_ready: model.configuration_ready,
            operations: model
                .capabilities
                .operations()
                .iter()
                .map(|operation| operation.as_str().to_owned())
                .collect(),
            features,
            upstream_validates_features: model.capabilities.upstream_validates_features(),
            context_window_tokens: presentation.and_then(|value| value.context_window_tokens()),
            max_output_tokens: model.capabilities.max_output_tokens(),
            hidden: presentation.is_some_and(|value| value.hidden()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogPageView {
    items: Vec<ModelView>,
    total: u64,
    config_revision: String,
    provider_generations: BTreeMap<String, String>,
    providers: Vec<String>,
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new().route("/api/admin/model-catalog", get(list::<S>))
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
        .model_catalog()
        .list(ModelCatalogQuery {
            page: query.page.unwrap_or(1),
            page_size: PageSize::new(query.page_size.unwrap_or(20))
                .map_err(|_| AdminError::bad_request("模型目录分页参数不合法"))?,
            search: query
                .search
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            provider: query
                .provider
                .map(ProviderKind::new)
                .transpose()
                .map_err(|_| AdminError::bad_request("供应商标识不合法"))?,
            source_kind: query.source_kind.map(Into::into),
            configuration_ready: query.configuration_ready,
        })
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(CatalogPageView {
            items: page.items.into_iter().map(ModelView::from).collect(),
            total: page.total,
            config_revision: page.config_revision.get().to_string(),
            provider_generations: page
                .provider_generations
                .into_iter()
                .map(|(provider, generation)| (provider.to_string(), generation.to_string()))
                .collect(),
            providers: page
                .providers
                .into_iter()
                .map(|provider| provider.to_string())
                .collect(),
        }),
    ))
}
