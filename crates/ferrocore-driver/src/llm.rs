//! Bounded Chat Completions SSE client. Errors never include server input or secrets.

use ferrocore_types::Message;
use reqwest::{header, Client, Method, Url};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    future::Future,
    net::IpAddr,
    time::{Duration, SystemTime},
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageStatus {
    Available(TokenUsage),
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LlmResponse {
    pub text: String,
    pub usage: UsageStatus,
    pub finish_reason: Option<String>,
}

impl Default for UsageStatus {
    fn default() -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    InvalidConfiguration,
    Network,
    HttpStatus(u16),
    Protocol,
    StreamInterrupted,
    ResponseLimitExceeded,
    Cancelled,
    Timeout,
    RetryAfterExceedsTimeout,
    Randomness,
}

impl LlmErrorKind {
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::Network => "network",
            Self::HttpStatus(_) => "http_status",
            Self::Protocol => "protocol",
            Self::StreamInterrupted => "stream_interrupted",
            Self::ResponseLimitExceeded => "response_limit_exceeded",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::RetryAfterExceedsTimeout => "retry_after_exceeds_timeout",
            Self::Randomness => "randomness",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub partial: LlmResponse,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            LlmErrorKind::HttpStatus(code) => write!(f, "LLM error: http_status ({code})"),
            other => write!(f, "LLM error: {}", other.reason_code()),
        }
    }
}

impl std::error::Error for LlmError {}

#[derive(Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_retries: u32,
    pub retry_base: Duration,
    pub retry_cap: Duration,
    pub max_response_bytes: usize,
}

impl fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("max_retries", &self.max_retries)
            .field("retry_base", &self.retry_base)
            .field("retry_cap", &self.retry_cap)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OriginKey {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl OriginKey {
    fn from_url(url: &Url) -> Option<Self> {
        let host = url.host_str()?.to_ascii_lowercase();
        Some(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host,
            port: url.port_or_known_default(),
        })
    }
}

pub struct LlmClient {
    client: Client,
    endpoint: Url,
    origin: OriginKey,
    model: String,
    key: String,
    retries: u32,
    base: Duration,
    cap: Duration,
    limit: usize,
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.endpoint.as_str())
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("retries", &self.retries)
            .field("base", &self.base)
            .field("cap", &self.cap)
            .field("limit", &self.limit)
            .finish()
    }
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Result<Self, LlmErrorKind> {
        let mut url = Url::parse(config.base_url.trim())
            .map_err(|_| LlmErrorKind::InvalidConfiguration)?;
        if !is_allowed_url(&url)
            || url.query().is_some()
            || url.fragment().is_some()
            || config.model.trim().is_empty()
            || config.api_key.trim().is_empty()
            || config.max_response_bytes == 0
            || config.retry_base.is_zero()
            || config.retry_cap.is_zero()
        {
            return Err(LlmErrorKind::InvalidConfiguration);
        }
        let path = format!("{}/chat/completions", url.path().trim_end_matches('/'));
        url.set_path(&path);
        let origin = OriginKey::from_url(&url).ok_or(LlmErrorKind::InvalidConfiguration)?;
        let mut builder = Client::builder().redirect(reqwest::redirect::Policy::none());
        if is_loopback_url(&url) {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|_| LlmErrorKind::InvalidConfiguration)?;
        Ok(Self {
            client,
            endpoint: url,
            origin,
            model: config.model,
            key: config.api_key,
            retries: config.max_retries,
            base: config.retry_base,
            cap: config.retry_cap,
            limit: config.max_response_bytes,
        })
    }

    pub fn endpoint_url(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Deadline is an absolute monotonic session/step deadline, if configured.
    /// Every failure carries the safely accumulated partial content for logging.
    pub async fn stream_chat(
        &self,
        messages: &[Message],
        cancel: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<LlmResponse, LlmError> {
        let request_messages: Vec<_> = messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "messages": request_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(LlmError {
                    kind: LlmErrorKind::Cancelled,
                    partial: LlmResponse::default(),
                });
            }
            if deadline.is_some_and(|end| Instant::now() >= end) {
                return Err(LlmError {
                    kind: LlmErrorKind::Timeout,
                    partial: LlmResponse::default(),
                });
            }

            let mut parser = Sse::new(self.limit);
            let response = match self.send_with_redirects(&body, cancel, deadline).await {
                Ok(response) => response,
                Err(SendFailure::Terminal(kind)) => {
                    return Err(LlmError {
                        kind,
                        partial: parser.response,
                    });
                }
                Err(SendFailure::RetryableNetwork) => {
                    if attempt >= self.retries {
                        return Err(LlmError {
                            kind: LlmErrorKind::Network,
                            partial: parser.response,
                        });
                    }
                    self.delay(attempt, None, cancel, deadline)
                        .await
                        .map_err(|kind| LlmError {
                            kind,
                            partial: parser.response.clone(),
                        })?;
                    attempt += 1;
                    continue;
                }
            };

            let status = response.status();
            if !status.is_success() {
                let code = status.as_u16();
                let kind = LlmErrorKind::HttpStatus(code);
                if !matches!(code, 408 | 429 | 500 | 502 | 503 | 504) || attempt >= self.retries {
                    return Err(LlmError {
                        kind,
                        partial: parser.response,
                    });
                }
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| parse_retry_after(v, SystemTime::now()));
                self.delay(attempt, retry_after, cancel, deadline)
                    .await
                    .map_err(|kind| LlmError {
                        kind,
                        partial: parser.response.clone(),
                    })?;
                attempt += 1;
                continue;
            }

            if let Some(content_type) = response.headers().get(header::CONTENT_TYPE) {
                let valid_type = content_type
                    .to_str()
                    .ok()
                    .map(|v| {
                        v.split(';')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case("text/event-stream")
                    })
                    .unwrap_or(false);
                if !valid_type {
                    return Err(LlmError {
                        kind: LlmErrorKind::Protocol,
                        partial: parser.response,
                    });
                }
            }

            let mut response = response;
            loop {
                match controlled(cancel, deadline, response.chunk()).await {
                    Err(kind) => {
                        return Err(LlmError {
                            kind,
                            partial: parser.response,
                        });
                    }
                    Ok(Ok(Some(chunk))) => match parser.feed(&chunk) {
                        Ok(true) => return Ok(parser.response),
                        Ok(false) => {}
                        Err(kind) => {
                            return Err(LlmError {
                                kind,
                                partial: parser.response,
                            });
                        }
                    },
                    Ok(Ok(None)) => {
                        if parser.delta_seen || parser.response.usage != UsageStatus::Unavailable || attempt >= self.retries {
                            return Err(LlmError {
                                kind: LlmErrorKind::StreamInterrupted,
                                partial: parser.response,
                            });
                        }
                        self.delay(attempt, None, cancel, deadline)
                            .await
                            .map_err(|kind| LlmError {
                                kind,
                                partial: parser.response.clone(),
                            })?;
                        attempt += 1;
                        break;
                    }
                    Ok(Err(_)) => {
                        if parser.delta_seen || parser.response.usage != UsageStatus::Unavailable || attempt >= self.retries {
                            return Err(LlmError {
                                kind: LlmErrorKind::StreamInterrupted,
                                partial: parser.response,
                            });
                        }
                        self.delay(attempt, None, cancel, deadline)
                            .await
                            .map_err(|kind| LlmError {
                                kind,
                                partial: parser.response.clone(),
                            })?;
                        attempt += 1;
                        break;
                    }
                }
            }
        }
    }

    async fn send_with_redirects(
        &self,
        body: &serde_json::Value,
        cancel: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<reqwest::Response, SendFailure> {
        let mut current_url = self.endpoint.clone();
        let mut method = Method::POST;
        let mut include_body = true;

        for _ in 0..=MAX_REDIRECTS {
            let mut req = self.client.request(method.clone(), current_url.clone());
            req = req.header(header::ACCEPT, "text/event-stream");
            req = req.bearer_auth(&self.key);
            if include_body {
                req = req.json(body);
            }

            let response = match controlled(cancel, deadline, req.send()).await {
                Err(kind) => return Err(SendFailure::Terminal(kind)),
                Ok(Err(err)) => {
                    if err.is_builder() {
                        return Err(SendFailure::Terminal(LlmErrorKind::InvalidConfiguration));
                    }
                    return Err(SendFailure::RetryableNetwork);
                }
                Ok(Ok(resp)) => resp,
            };

            if !response.status().is_redirection() {
                return Ok(response);
            }

            let status_code = response.status().as_u16();
            let Some(location) = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return Err(SendFailure::Terminal(LlmErrorKind::Protocol));
            };
            let next_url = current_url
                .join(location)
                .map_err(|_| SendFailure::Terminal(LlmErrorKind::Protocol))?;
            if !is_allowed_url(&next_url) {
                return Err(SendFailure::Terminal(LlmErrorKind::InvalidConfiguration));
            }
            let Some(next_origin) = OriginKey::from_url(&next_url) else {
                return Err(SendFailure::Terminal(LlmErrorKind::InvalidConfiguration));
            };
            // Reject cross-origin redirects before sending credentials or the POST body (task/history).
            if next_origin != self.origin {
                return Err(SendFailure::Terminal(LlmErrorKind::InvalidConfiguration));
            }

            match status_code {
                307 | 308 => {}
                301..=303 => {
                    method = Method::GET;
                    include_body = false;
                }
                _ => return Err(SendFailure::Terminal(LlmErrorKind::Protocol)),
            }
            current_url = next_url;
        }

        Err(SendFailure::Terminal(LlmErrorKind::Protocol))
    }

    async fn delay(
        &self,
        attempt: u32,
        retry_after: Option<Duration>,
        cancel: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<(), LlmErrorKind> {
        let factor = 1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
        let ceiling = self.base.saturating_mul(factor).min(self.cap);
        let jitter = full_jitter_duration(ceiling)?;
        if let (Some(ra), Some(end)) = (retry_after, deadline) {
            let now = Instant::now();
            if now >= end {
                return Err(LlmErrorKind::Timeout);
            }
            if ra >= end.saturating_duration_since(now) {
                return Err(LlmErrorKind::RetryAfterExceedsTimeout);
            }
        }
        let delay = jitter.max(retry_after.unwrap_or_default());
        if let Some(end) = deadline {
            let now = Instant::now();
            if now >= end || delay >= end.saturating_duration_since(now) {
                return Err(LlmErrorKind::Timeout);
            }
        }
        if !delay.is_zero() {
            controlled(cancel, deadline, tokio::time::sleep(delay)).await?;
        }
        Ok(())
    }
}

enum SendFailure {
    RetryableNetwork,
    Terminal(LlmErrorKind),
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .map(|ip| match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => {
                    v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
                }
            })
            .unwrap_or(false),
        None => false,
    }
}

fn is_allowed_url(url: &Url) -> bool {
    let scheme_ok =
        url.scheme() == "https" || (url.scheme() == "http" && is_loopback_url(url));
    scheme_ok
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn full_jitter_duration(ceiling: Duration) -> Result<Duration, LlmErrorKind> {
    let max_nanos = ceiling.as_nanos();
    let nanos = uniform_u128_inclusive(max_nanos)?;
    Ok(Duration::new(
        (nanos / 1_000_000_000) as u64,
        (nanos % 1_000_000_000) as u32,
    ))
}

fn uniform_u128_inclusive(max: u128) -> Result<u128, LlmErrorKind> {
    if max == 0 {
        return Ok(0);
    }
    if max == u128::MAX {
        let mut buf = [0u8; 16];
        getrandom::getrandom(&mut buf).map_err(|_| LlmErrorKind::Randomness)?;
        return Ok(u128::from_le_bytes(buf));
    }
    let range = max + 1;
    let limit = (u128::MAX / range) * range;
    loop {
        let mut buf = [0u8; 16];
        getrandom::getrandom(&mut buf).map_err(|_| LlmErrorKind::Randomness)?;
        let value = u128::from_le_bytes(buf);
        if value < limit {
            return Ok(value % range);
        }
    }
}

async fn controlled<F: Future>(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    future: F,
) -> Result<F::Output, LlmErrorKind> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(LlmErrorKind::Cancelled),
        _ = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        } => Err(LlmErrorKind::Timeout),
        value = future => Ok(value),
    }
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
            .or(Some(Duration::MAX));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}

#[derive(Deserialize)]
struct RawUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: Option<u64>,
}

struct Sse {
    line: Vec<u8>,
    data: Vec<u8>,
    limit: usize,
    started: bool,
    bom_buf: [u8; 3],
    bom_len: usize,
    cr: bool,
    delta_seen: bool,
    response: LlmResponse,
}

impl Sse {
    fn new(limit: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            limit,
            started: false,
            bom_buf: [0; 3],
            bom_len: 0,
            cr: false,
            delta_seen: false,
            response: LlmResponse::default(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<bool, LlmErrorKind> {
        self.feed_inner(bytes)
    }

    fn feed_inner(&mut self, bytes: &[u8]) -> Result<bool, LlmErrorKind> {
        for &byte in bytes {
            if !self.started {
                const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
                if byte == BOM[self.bom_len] {
                    self.bom_buf[self.bom_len] = byte;
                    self.bom_len += 1;
                    if self.bom_len == 3 {
                        self.bom_len = 0;
                        self.started = true;
                    }
                    continue;
                }
                self.started = true;
                if self.bom_len > 0 {
                    let buffered = self.bom_buf;
                    let len = self.bom_len;
                    self.bom_len = 0;
                    for &prev in &buffered[..len] {
                        if self.feed_byte(prev)? {
                            return Ok(true);
                        }
                    }
                }
            }
            if self.feed_byte(byte)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn feed_byte(&mut self, byte: u8) -> Result<bool, LlmErrorKind> {
        if self.cr {
            self.cr = false;
            if byte == b'\n' {
                return Ok(false);
            }
        }
        if byte == b'\r' || byte == b'\n' {
            self.cr = byte == b'\r';
            if self.line()? {
                return Ok(true);
            }
        } else {
            if self.line.len() >= self.limit {
                return Err(LlmErrorKind::ResponseLimitExceeded);
            }
            self.line.push(byte);
        }
        Ok(false)
    }

    fn line(&mut self) -> Result<bool, LlmErrorKind> {
        let line = std::mem::take(&mut self.line);
        let text = std::str::from_utf8(&line).map_err(|_| LlmErrorKind::Protocol)?;
        if text.is_empty() {
            if self.data.is_empty() {
                return Ok(false);
            }
            let data = std::mem::take(&mut self.data);
            let text = std::str::from_utf8(&data).map_err(|_| LlmErrorKind::Protocol)?;
            if text.trim() == "[DONE]" {
                return Ok(true);
            }
            if text.trim().is_empty() {
                return Ok(false);
            }
            let value: serde_json::Value =
                serde_json::from_str(text).map_err(|_| LlmErrorKind::Protocol)?;
            let obj = value.as_object().ok_or(LlmErrorKind::Protocol)?;
            if obj.get("error").is_some_and(|v| !v.is_null()) {
                return Err(LlmErrorKind::Protocol);
            }
            let choices = if let Some(value) = obj.get("choices") {
                let choices = value.as_array().ok_or(LlmErrorKind::Protocol)?;
                if choices.len() > 1 {
                    return Err(LlmErrorKind::Protocol);
                }
                if let Some(choice) = choices.first() {
                    let choice = choice.as_object().ok_or(LlmErrorKind::Protocol)?;
                    if choice.get("index").is_some_and(|index| index.as_u64() != Some(0)) {
                        return Err(LlmErrorKind::Protocol);
                    }
                }
                Some(choices)
            } else {
                None
            };
            let has_usage = if let Some(usage) = obj.get("usage").filter(|v| !v.is_null()) {
                let raw: RawUsage =
                    serde_json::from_value(usage.clone()).map_err(|_| LlmErrorKind::Protocol)?;
                let sum = raw
                    .prompt_tokens
                    .checked_add(raw.completion_tokens)
                    .ok_or(LlmErrorKind::Protocol)?;
                if raw.total_tokens.is_some_and(|total| total != sum) {
                    return Err(LlmErrorKind::Protocol);
                }
                self.response.usage = UsageStatus::Available(TokenUsage {
                    prompt_tokens: raw.prompt_tokens,
                    completion_tokens: raw.completion_tokens,
                    total_tokens: sum,
                });
                true
            } else {
                false
            };
            if let Some(choices) = choices {
                for choice in choices {
                    if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
                        self.response.finish_reason =
                            Some(reason.as_str().ok_or(LlmErrorKind::Protocol)?.to_owned());
                    }
                    if let Some(delta) = choice.get("delta").filter(|v| !v.is_null()) {
                        let delta = delta.as_object().ok_or(LlmErrorKind::Protocol)?;
                        self.delta_seen = true;
                        if let Some(content) = delta.get("content").filter(|v| !v.is_null()) {
                            let content = content.as_str().ok_or(LlmErrorKind::Protocol)?;
                            if content.len() > self.limit.saturating_sub(self.response.text.len()) {
                                return Err(LlmErrorKind::ResponseLimitExceeded);
                            }
                            self.response.text.push_str(content);
                        }
                    }
                }
            } else if !has_usage {
                return Err(LlmErrorKind::Protocol);
            }
            return Ok(false);
        }
        if text.starts_with(':') {
            return Ok(false);
        }
        let (field, mut value) = text.split_once(':').unwrap_or((text, ""));
        if let Some(rest) = value.strip_prefix(' ') {
            value = rest;
        }
        if field == "data" {
            let extra = value.len() + usize::from(!self.data.is_empty());
            if extra > self.limit.saturating_sub(self.data.len()) {
                return Err(LlmErrorKind::ResponseLimitExceeded);
            }
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(value.as_bytes());
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn test_config(base_url: String) -> LlmConfig {
        LlmConfig {
            base_url,
            model: "test-model".to_owned(),
            api_key: "super-secret-api-key-123".to_owned(),
            max_retries: 3,
            retry_base: Duration::from_millis(5),
            retry_cap: Duration::from_millis(20),
            max_response_bytes: 4096,
        }
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        let header_end = loop {
            let n = stream.read(&mut tmp).await.unwrap();
            assert!(n > 0, "unexpected EOF reading request headers");
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_len = headers
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                if k.eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        while buf.len() < header_end + content_len {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    #[test]
    fn origin_validation_allows_https_and_loopback_http_only() {
        assert!(LlmClient::new(test_config("https://api.example.com/v1".into())).is_ok());
        assert!(LlmClient::new(test_config("http://127.0.0.1:8080/v1".into())).is_ok());
        assert!(LlmClient::new(test_config("http://localhost:8080".into())).is_ok());
        assert!(LlmClient::new(test_config("http://[::1]:8080/v1".into())).is_ok());
        assert!(LlmClient::new(test_config("http://[::ffff:127.0.0.1]:8080".into())).is_ok());

        for bad in [
            "http://api.example.com/v1",
            "http://192.168.1.10:8080/v1",
            "http://0.0.0.0:8080/v1",
            "http://localhost.evil.com:8080/v1",
            "ftp://api.example.com/v1",
            "https://user:pass@api.example.com/v1",
            "https://api.example.com/v1?x=1",
            "https://api.example.com/v1#frag",
        ] {
            assert_eq!(
                LlmClient::new(test_config(bad.into())).unwrap_err(),
                LlmErrorKind::InvalidConfiguration,
                "expected rejection for {bad}"
            );
        }
    }

    #[test]
    fn secret_is_redacted_in_debug_and_display() {
        let cfg = test_config("https://api.example.com/v1".into());
        assert!(!format!("{cfg:?}").contains("super-secret-api-key-123"));
        let client = LlmClient::new(cfg).unwrap();
        assert!(!format!("{client:?}").contains("super-secret-api-key-123"));
        assert_eq!(
            client.endpoint_url(),
            "https://api.example.com/v1/chat/completions"
        );
        let err = LlmError {
            kind: LlmErrorKind::ResponseLimitExceeded,
            partial: LlmResponse::default(),
        };
        assert!(err.to_string().contains("response_limit_exceeded"));
        assert!(!err.to_string().contains("super-secret-api-key-123"));
    }

    #[test]
    fn sse_parser_handles_every_byte_boundary_crlf_utf8_comments_and_multiline_data() {
        let payload = concat!(
            "\u{FEFF}: keepalive\r",
            "event: message\r\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Merhaba \"}}]}\r\r",
            "data: {\"choices\":\n",
            "data: [{\"delta\":{\"content\":\"dünya 🦀\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\n",
            "data: \"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4}}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        )
        .as_bytes();

        for chunk_size in [1usize, 2, 3, 5, 17, payload.len()] {
            let mut parser = Sse::new(4096);
            let mut done = false;
            for chunk in payload.chunks(chunk_size) {
                done = parser.feed(chunk).unwrap();
                if done {
                    break;
                }
            }
            assert!(done, "chunk_size={chunk_size}");
            assert_eq!(parser.response.text, "Merhaba dünya 🦀");
            assert_eq!(parser.response.finish_reason.as_deref(), Some("stop"));
            assert_eq!(
                parser.response.usage,
                UsageStatus::Available(TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 4,
                    total_tokens: 14,
                })
            );
        }
    }

    #[test]
    fn sse_parser_rejects_inconsistent_usage_and_enforces_response_limit() {
        let bad_usage = b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":99}}\n\n";
        let mut parser = Sse::new(4096);
        assert_eq!(parser.feed(bad_usage), Err(LlmErrorKind::Protocol));

        // Comments and small valid events whose cumulative wire size exceeds limit must still succeed
        // when each line, event payload, and accumulated content fits within `limit`.
        let mut many = String::new();
        for _ in 0..8 {
            many.push_str(": keepalive-comment\n\n");
        }
        many.push_str("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n");
        many.push_str("data: [DONE]\n\n");
        let mut seq_parser = Sse::new(64);
        let mut seq_done = false;
        for chunk in many.as_bytes().chunks(5) {
            seq_done = seq_parser.feed(chunk).unwrap();
            if seq_done {
                break;
            }
        }
        assert!(seq_done);
        assert_eq!(seq_parser.response.text, "ok");

        // 1) Line limit exceeded across chunk splits
        let long_line = format!(": {}\n\n", "x".repeat(65));
        for chunk_size in [1usize, 7, long_line.len()] {
            let mut p = Sse::new(64);
            let mut hit = false;
            for chunk in long_line.as_bytes().chunks(chunk_size) {
                if p.feed(chunk) == Err(LlmErrorKind::ResponseLimitExceeded) {
                    hit = true;
                    break;
                }
            }
            assert!(hit, "line limit not enforced for chunk_size={chunk_size}");
        }

        // 2) Accumulated SSE event `data` limit exceeded across multiline data
        let multiline = "data: 12345678901234567890\ndata: 12345678901234567890\ndata: 12345678901234567890\n\n";
        for chunk_size in [1usize, 9, multiline.len()] {
            let mut p = Sse::new(50);
            let mut hit = false;
            for chunk in multiline.as_bytes().chunks(chunk_size) {
                if p.feed(chunk) == Err(LlmErrorKind::ResponseLimitExceeded) {
                    hit = true;
                    break;
                }
            }
            assert!(hit, "data limit not enforced for chunk_size={chunk_size}");
        }

        // 3) Accumulated `response.text` limit exceeded across multiple small events (line/data fit, content overflows)
        let content_events = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"123456789012345678901234567890\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"123456789012345678901234567890\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"123456789012345678901234567890\"}}]}\n\n",
        );
        for chunk_size in [1usize, 11, content_events.len()] {
            let mut limited = Sse::new(80);
            let mut hit_limit = false;
            for chunk in content_events.as_bytes().chunks(chunk_size) {
                if let Err(LlmErrorKind::ResponseLimitExceeded) = limited.feed(chunk) {
                    hit_limit = true;
                    break;
                }
            }
            assert!(hit_limit, "content limit not enforced for chunk_size={chunk_size}");
            assert_eq!(limited.response.text, "123456789012345678901234567890123456789012345678901234567890");
        }
    }

    #[test]
    fn retry_after_parses_seconds_and_http_date() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(parse_retry_after("42", now), Some(Duration::from_secs(42)));
        let future = httpdate::fmt_http_date(now + Duration::from_secs(15));
        assert_eq!(parse_retry_after(&future, now), Some(Duration::from_secs(15)));
        let past = httpdate::fmt_http_date(now - Duration::from_secs(10));
        assert_eq!(parse_retry_after(&past, now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("invalid", now), None);
    }

    #[test]
    fn full_jitter_stays_within_exponential_cap() {
        let cap = Duration::from_millis(80);
        let base = Duration::from_millis(15);
        for attempt in 0..5 {
            let expected_max = base
                .saturating_mul(1u32.checked_shl(attempt).unwrap())
                .min(cap);
            for _ in 0..32 {
                let d = full_jitter_duration(expected_max).unwrap();
                assert!(d <= expected_max);
            }
        }
    }

    #[tokio::test]
    async fn loopback_rejects_multiple_or_nonzero_choices() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"zero\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" implicit\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"safe\"}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"bad\"}},{\"index\":1,\"delta\":{\"content\":\"other\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"safe\"}}]}\n\ndata: {\"choices\":[{\"index\":1,\"delta\":{\"content\":\"bad\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":null,\"delta\":{\"content\":\"bad\"}}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":\"0\",\"delta\":{\"content\":\"bad\"}}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[null]}\n\ndata: [DONE]\n\n",
        ];
        let server = tokio::spawn(async move {
            for body in bodies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let _ = read_http_request(&mut sock).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                sock.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LlmClient::new(test_config(format!("http://{addr}/v1"))).unwrap();
        let messages = [Message { role: "user".into(), content: "hi".into() }];
        let cancel = CancellationToken::new();
        let response = client.stream_chat(&messages, &cancel, None).await.unwrap();
        assert_eq!(response.text, "zero implicit");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert_eq!(response.usage, UsageStatus::Available(TokenUsage {
            prompt_tokens: 1, completion_tokens: 2, total_tokens: 3,
        }));
        for expected_partial in ["safe", "safe", "", "", ""] {
            let error = client.stream_chat(&messages, &cancel, None).await.unwrap_err();
            assert_eq!(error.kind, LlmErrorKind::Protocol);
            assert_eq!(error.partial.text, expected_partial);
            assert_eq!(error.partial.finish_reason, None);
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cross_origin_redirect_is_rejected_while_same_origin_succeeds() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source_addr = source.local_addr().unwrap();
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_hits_clone = Arc::clone(&target_hits);

        let target_task = tokio::spawn(async move {
            if let Ok((_sock, _)) = target.accept().await {
                target_hits_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        let source_task = tokio::spawn(async move {
            // 1) Cross-origin redirect must be rejected without contacting target.
            let (mut sock, _) = source.accept().await.unwrap();
            let req = read_http_request(&mut sock).await;
            assert!(req.lines().any(|l| l.eq_ignore_ascii_case("authorization: Bearer super-secret-api-key-123")));
            let resp = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_addr}/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();

            // 2) Same-origin redirect succeeds and retains auth + body.
            let (mut sock, _) = source.accept().await.unwrap();
            let req = read_http_request(&mut sock).await;
            assert!(req.starts_with("POST /v1/chat/completions "));
            let resp = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{source_addr}/v2/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();

            let (mut sock, _) = source.accept().await.unwrap();
            let req = read_http_request(&mut sock).await;
            assert!(req.starts_with("POST /v2/chat/completions "));
            assert!(req.lines().any(|l| l.eq_ignore_ascii_case("authorization: Bearer super-secret-api-key-123")));
            assert!(req.contains("\"content\":\"hi\""));
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"same-origin-ok\"}}]}\n\ndata: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });

        let client = LlmClient::new(test_config(format!("http://{source_addr}/v1"))).unwrap();
        let msgs = [Message {
            role: "user".into(),
            content: "hi".into(),
        }];
        let cancel = CancellationToken::new();
        let err = client.stream_chat(&msgs, &cancel, None).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::InvalidConfiguration);
        assert_eq!(target_hits.load(Ordering::SeqCst), 0, "cross-origin target must never be contacted");
        target_task.abort();

        let ok = client.stream_chat(&msgs, &cancel, None).await.unwrap();
        assert_eq!(ok.text, "same-origin-ok");
        source_task.await.unwrap();
    }

    #[tokio::test]
    async fn retries_503_then_succeeds_and_fails_fast_on_401() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let _ = read_http_request(&mut sock).await;
                if attempt == 0 {
                    sock.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                } else {
                    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    sock.write_all(resp.as_bytes()).await.unwrap();
                }
            }
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut sock).await;
            sock.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });

        let client = LlmClient::new(test_config(format!("http://{addr}/v1"))).unwrap();
        let msgs = [Message {
            role: "user".into(),
            content: "hi".into(),
        }];
        let cancel = CancellationToken::new();
        let ok = client.stream_chat(&msgs, &cancel, None).await.unwrap();
        assert_eq!(ok.text, "ok");
        assert_eq!(ok.usage, UsageStatus::Unavailable);

        let err = client.stream_chat(&msgs, &cancel, None).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::HttpStatus(401));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn no_retry_after_first_delta_and_retry_after_exceeding_deadline_aborts_immediately() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // 1) Partial stream after delta -> should NOT be retried (only 1 connection)
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut sock).await;
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            drop(sock);

            // 2) 429 with Retry-After: 120 when deadline is 5s away -> should NOT sleep 5s
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut sock).await;
            sock.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });

        let client = LlmClient::new(test_config(format!("http://{addr}/v1"))).unwrap();
        let msgs = [Message {
            role: "user".into(),
            content: "hi".into(),
        }];
        let cancel = CancellationToken::new();

        let interrupted = client.stream_chat(&msgs, &cancel, None).await.unwrap_err();
        assert_eq!(interrupted.kind, LlmErrorKind::StreamInterrupted);
        assert_eq!(interrupted.partial.text, "partial");

        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);
        let rate_err = client
            .stream_chat(&msgs, &cancel, Some(deadline))
            .await
            .unwrap_err();
        assert_eq!(rate_err.kind, LlmErrorKind::RetryAfterExceedsTimeout);
        assert!(start.elapsed() < Duration::from_secs(1));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn role_only_or_empty_delta_before_disconnect_does_not_retry() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hits);

        let server = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits_clone.fetch_add(1, Ordering::SeqCst);
                let _ = read_http_request(&mut sock).await;
                let body = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
        });

        let client = LlmClient::new(test_config(format!("http://{addr}/v1"))).unwrap();
        let msgs = [Message {
            role: "user".into(),
            content: "hi".into(),
        }];
        let err = client
            .stream_chat(&msgs, &CancellationToken::new(), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::StreamInterrupted);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "must not retry after receiving any delta");
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_aborts_active_stream_and_preserves_partial_text() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut sock).await;
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"before-cancel\"}}]}\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            let _ = sent_tx.send(());
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = LlmClient::new(test_config(format!("http://{addr}/v1"))).unwrap();
        let msgs = [Message {
            role: "user".into(),
            content: "hi".into(),
        }];
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            let _ = sent_rx.await;
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel_clone.cancel();
        });

        let err = client.stream_chat(&msgs, &cancel, None).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::Cancelled);
        assert_eq!(err.partial.text, "before-cancel");
        server.abort();
    }
}
