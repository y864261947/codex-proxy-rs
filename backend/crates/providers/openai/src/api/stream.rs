//! Responses 渠道的单次 HTTP 发送、取消与 SSE 观测。

use std::{
    future::Future,
    time::{Duration, SystemTime},
};

use futures::StreamExt as _;
use gateway_core::{
    engine::{AttemptContext, provider::EventStream},
    error::{ProviderError, ProviderErrorKind},
    event::{
        GatewayEvent, ProviderEvent, ProviderResponseObservation, ProviderResponseTimings,
        UpstreamHttpVersion,
    },
    routing::UpstreamModelId,
    upstream::{OpaqueUpstreamValue, UpstreamSendState},
};
use reqwest::{
    Client,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap},
};
use serde_json::{Map, Value};

use super::{config::ApiChannelConfig, provider::transport};
use crate::transport::canonical::{
    CodexCanonicalDecoder, CodexCanonicalError, CodexCanonicalOutcome,
};

pub(super) fn response_stream(
    client: Client,
    config: ApiChannelConfig,
    body: Map<String, Value>,
    model: UpstreamModelId,
    context: AttemptContext,
) -> EventStream {
    Box::pin(async_stream::try_stream! {
        // 构造请求无 I/O；只有 Core 持久化 attempt 并首次 poll 后才开始发送。
        let mut request = client.post(config.responses_url()).bearer_auth(config.api_key())
            .header(ACCEPT, "text/event-stream").json(&body);
        if let Some(organization) = config.organization() { request = request.header("openai-organization", organization); }
        if let Some(project) = config.project() { request = request.header("openai-project", project); }
        let request = request.build().map_err(|_| error(ProviderErrorKind::InvalidRequest, UpstreamSendState::NotSent))?;
        if context.cancellation().is_cancelled() { Err(error(ProviderErrorKind::Cancelled, UpstreamSendState::NotSent))?; }
        if context.deadline() <= SystemTime::now() { Err(error(ProviderErrorKind::Timeout, UpstreamSendState::NotSent))?; }
        // 一旦 send future 开始，超时/取消按不确定发送处理，不能假设上游没收到。
        let response = await_request(&context, UpstreamSendState::Ambiguous, client.execute(request)).await?
            .map_err(|failure| network_error(&failure, UpstreamSendState::Ambiguous))?;
        let status = response.status();
        let mut timings = ProviderResponseTimings { headers_ms: Some(elapsed(&context)), ..Default::default() };
        let mut observation = ProviderResponseObservation::new(transport()).with_status_code(status.as_u16());
        if let Some(version) = UpstreamHttpVersion::parse(&format!("{:?}", response.version())) {
            observation = observation.with_http_version(version);
        }
        if let Some(id) = response.headers().get("x-request-id").and_then(|value| value.to_str().ok()) {
            observation = observation.with_request_id(OpaqueUpstreamValue::new(id));
        }
        yield ProviderEvent::observation(observation.clone().with_timings(timings));
        if !status.is_success() { Err(status_error(status.as_u16(), response.headers()))?; }
        let sse = response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream")));
        if !sse { Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?; }

        let mut decoder = CodexCanonicalDecoder::new(model.as_str()).with_raw_sse_passthrough().with_usage_only();
        let mut chunks = response.bytes_stream();
        let mut completed = false;
        loop {
            let next = await_request(&context, UpstreamSendState::Sent, chunks.next()).await?;
            let eof = next.is_none();
            let outcome = match next {
                Some(Ok(bytes)) => decoder.push(&bytes),
                Some(Err(failure)) => Err(network_error(&failure, UpstreamSendState::Sent))?,
                None => decoder.finish(),
            };
            let signals = decoder.take_timing_signals();
            let now = elapsed(&context);
            if signals.protocol_progress { timings.first_event_ms.get_or_insert(now); }
            if signals.semantic_output { timings.first_token_ms.get_or_insert(now); }
            if signals.text_output { timings.first_text_ms.get_or_insert(now); }
            if signals.reasoning_output { timings.first_reasoning_ms.get_or_insert(now); }
            if let Some(tier) = decoder.response_service_tier() {
                observation = observation.with_service_tier_if_valid(tier);
            }
            yield ProviderEvent::observation(observation.clone().with_timings(timings));
            let (events, failure) = match outcome {
                CodexCanonicalOutcome::Events(events) => (events, None),
                CodexCanonicalOutcome::Failed(failure) => {
                    let (events, failure, _) = failure.into_parts();
                    (events, Some(failure))
                }
            };
            for event in events {
                completed |= event.canonical_facts().iter().any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                yield event;
            }
            if let Some(failure) = failure {
                Err(match failure {
                    CodexCanonicalError::Protocol(error) => error,
                    CodexCanonicalError::Upstream(failure) => {
                        let mut error = failure.explicit_status_code.map_or_else(
                            || error(ProviderErrorKind::Unavailable, UpstreamSendState::Sent),
                            |status| status_error(status, &HeaderMap::new()),
                        );
                        if let Some(seconds) = failure.retry_after_seconds { error = error.with_retry_after(Duration::from_secs(seconds)); }
                        error
                    }
                })?;
            }
            if completed { break; }
            if eof { Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?; }
        }
    })
}

async fn await_request<T>(
    context: &AttemptContext,
    state: UpstreamSendState,
    future: impl Future<Output = T>,
) -> Result<T, ProviderError> {
    let remaining = context
        .deadline()
        .duration_since(SystemTime::now())
        .map_err(|_| error(ProviderErrorKind::Timeout, state))?;
    tokio::select! {
        biased;
        _ = context.cancellation().cancelled() => Err(error(ProviderErrorKind::Cancelled, state)),
        _ = tokio::time::sleep(remaining) => Err(error(ProviderErrorKind::Timeout, state)),
        result = future => Ok(result),
    }
}

fn status_error(status: u16, headers: &HeaderMap) -> ProviderError {
    let kind = match status {
        400 | 404 | 409 | 422 => ProviderErrorKind::InvalidRequest,
        401 => ProviderErrorKind::Unauthorized,
        403 => ProviderErrorKind::PermissionDenied,
        408 | 504 => ProviderErrorKind::Timeout,
        429 => ProviderErrorKind::RateLimited,
        _ => ProviderErrorKind::Unavailable,
    };
    let mut result = error(kind, UpstreamSendState::Sent).with_status(status);
    if let Some(seconds) = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
    {
        result = result.with_retry_after(Duration::from_secs(seconds));
    }
    if let Some(id) = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
    {
        result = result.with_upstream_request_id(OpaqueUpstreamValue::new(id));
    }
    result
}

fn network_error(failure: &reqwest::Error, fallback: UpstreamSendState) -> ProviderError {
    let state = if failure.is_connect() {
        UpstreamSendState::NotSent
    } else {
        fallback
    };
    error(
        if failure.is_timeout() {
            ProviderErrorKind::Timeout
        } else {
            ProviderErrorKind::Transport
        },
        state,
    )
}
fn error(kind: ProviderErrorKind, state: UpstreamSendState) -> ProviderError {
    ProviderError::new(kind, state)
}
fn elapsed(context: &AttemptContext) -> u64 {
    u64::try_from(context.timing_started_at().elapsed().as_millis()).unwrap_or(u64::MAX)
}
