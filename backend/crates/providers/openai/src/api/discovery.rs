use std::{collections::BTreeSet, time::Duration};

use gateway_admin::{
    model::channels::DiscoveredChannelModels,
    ports::provider::{ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::routing::UpstreamModelId;
use serde::Deserialize;
use serde_json::Value;

use super::config::ApiChannelConfig;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODELS: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelList {
    object: String,
    data: Vec<ModelEntry>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next: Value,
    #[serde(default)]
    next_page: Value,
    #[serde(default)]
    next_cursor: Value,
    #[serde(default)]
    links: Value,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub(crate) async fn discover(
    config: ApiChannelConfig,
) -> Result<DiscoveredChannelModels, ProviderAdminError> {
    let client = crate::transport::build_reqwest_client()
        .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Internal))?;
    let mut request = client
        .get(config.models_url())
        .timeout(Duration::from_secs(15))
        .bearer_auth(config.api_key())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(value) = config.organization() {
        request = request.header("OpenAI-Organization", value);
    }
    if let Some(value) = config.project() {
        request = request.header("OpenAI-Project", value);
    }
    let mut response = request.send().await.map_err(|_| unavailable())?;
    if response.status() != reqwest::StatusCode::OK
        || response.headers().contains_key(reqwest::header::LINK)
    {
        return Err(bad_gateway());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(bad_gateway());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if chunk.len() > MAX_BYTES - body.len() {
            return Err(bad_gateway());
        }
        body.extend_from_slice(&chunk);
    }
    let list: ModelList = serde_json::from_slice(&body).map_err(|_| bad_gateway())?;
    if list.object != "list"
        || list.has_more
        || list.data.len() > MAX_MODELS
        || [&list.next, &list.next_page, &list.next_cursor, &list.links]
            .iter()
            .any(|value| !value.is_null())
    {
        return Err(bad_gateway());
    }
    let mut discovered = BTreeSet::new();
    for entry in list.data {
        if entry.id.trim() != entry.id {
            return Err(bad_gateway());
        }
        let model = UpstreamModelId::new(entry.id).map_err(|_| bad_gateway())?;
        if !discovered.insert(model) {
            return Err(bad_gateway());
        }
    }
    Ok(DiscoveredChannelModels {
        configured: config.models().clone(),
        discovered,
    })
}

fn bad_gateway() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::BadGateway)
}

fn unavailable() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable)
}
