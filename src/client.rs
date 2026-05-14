//! HTTP client wrapper for the Lynxx gateway API.
//!
//! Every gateway response uses a standard envelope:
//!
//! ```json
//! {
//!   "transactionUrn": "urn:...:...",
//!   "status": "SUCCESS" | "FAILED",
//!   "responseMessage": "...",
//!   "responseKey": "OK" | "VALIDATION_ERROR" | ...,
//!   "errors": [],
//!   "timestamp": "2026-03-31T12:00:00+05:30",
//!   "metadata": {},
//!   "data": {} | [],
//!   "reference_urn": "..."
//! }
//! ```
//!
//! The client unwraps this transparently: callers get the `data` payload on
//! success and a rich `ApiError` on failure.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

// ---------------------------------------------------------------------------
// Standard response envelope.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct Envelope {
    #[serde(default)]
    pub transaction_urn: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub response_message: Option<String>,
    #[serde(default)]
    pub response_key: Option<String>,
    #[serde(default)]
    pub errors: Vec<Json>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub metadata: Json,
    #[serde(default)]
    pub data: Json,
    #[serde(default)]
    pub reference_urn: Option<String>,
}

impl Envelope {
    fn is_envelope(v: &Json) -> bool {
        v.get("transactionUrn").is_some() || v.get("responseKey").is_some()
    }

    fn is_failed(&self) -> bool {
        matches!(self.status.as_deref(), Some("FAILED"))
    }
}

// ---------------------------------------------------------------------------
// API error type.
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)]
pub struct ApiError {
    pub method: &'static str,
    pub url: String,
    pub status: reqwest::StatusCode,
    pub body: String,
    pub response_key: Option<String>,
    pub message: Option<String>,
    pub errors: Vec<Json>,
    pub transaction_urn: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = self.message.as_deref().unwrap_or_else(|| {
            if self.body.is_empty() {
                self.status.canonical_reason().unwrap_or("error")
            } else {
                self.body.as_str()
            }
        });
        write!(
            f,
            "{} {} → {}: {}",
            self.method,
            self.url,
            self.status,
            msg.trim()
        )?;
        if let Some(key) = &self.response_key {
            write!(f, " [{key}]")?;
        }
        Ok(())
    }
}
impl std::error::Error for ApiError {}

// ---------------------------------------------------------------------------
// Envelope processing.
// ---------------------------------------------------------------------------

/// Try to parse the response body as an envelope.  If it looks like one,
/// return the parsed `Envelope`; otherwise return `None` (the response is
/// a plain value or non-JSON).
fn try_parse_envelope(text: &str) -> Option<Envelope> {
    let v: Json = serde_json::from_str(text).ok()?;
    if !Envelope::is_envelope(&v) {
        return None;
    }
    serde_json::from_value(v).ok()
}

/// Extract the human-readable error message from a response body, trying
/// the standard envelope fields first and then common ad-hoc keys.
pub fn parse_error_message(body: &str) -> Option<String> {
    let v: Json = serde_json::from_str(body).ok()?;
    for key in ["responseMessage", "error", "message"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.into());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Client.
// ---------------------------------------------------------------------------

pub struct Client {
    pub(crate) base: String,
    pub(crate) http: reqwest::Client,
    pub(crate) api_key: Option<String>,
}

impl Client {
    pub fn new(base: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
            api_key,
        }
    }

    // -- private helpers --

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.header("X-API-Key", key),
            None => req,
        }
    }

    async fn request_raw(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<(reqwest::StatusCode, String)> {
        let url = format!("{}{}", self.base, path);
        let req = self.http.request(method.clone(), &url);
        let resp = self
            .apply_auth(req)
            .send()
            .await
            .with_context(|| format!("{method} {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    async fn send_body<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<(reqwest::StatusCode, String)> {
        let url = format!("{}{}", self.base, path);
        let req = self.http.request(method.clone(), &url).json(body);
        let resp = self
            .apply_auth(req)
            .send()
            .await
            .with_context(|| format!("{method} {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// Check for HTTP-level failure *or* envelope-level `"status": "FAILED"`.
    /// Returns `Ok(data_payload_string)` on success.
    fn check_response(
        method: &'static str,
        url: &str,
        http_status: reqwest::StatusCode,
        text: String,
    ) -> Result<String> {
        if let Some(env) = try_parse_envelope(&text) {
            if !http_status.is_success() || env.is_failed() {
                return Err(ApiError {
                    method,
                    url: url.to_string(),
                    status: http_status,
                    response_key: env.response_key,
                    message: env.response_message,
                    errors: env.errors,
                    transaction_urn: env.transaction_urn,
                    body: text,
                }
                .into());
            }
            let data = if env.data.is_null() {
                "null".to_string()
            } else {
                env.data.to_string()
            };
            return Ok(data);
        }

        // Not an envelope — fall back to plain HTTP status check.
        if !http_status.is_success() {
            return Err(ApiError {
                method,
                url: url.to_string(),
                status: http_status,
                response_key: None,
                message: parse_error_message(&text),
                errors: vec![],
                transaction_urn: None,
                body: text,
            }
            .into());
        }
        Ok(text)
    }

    // -- public API --

    pub async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.request_raw(reqwest::Method::GET, path).await?;
        let payload = Self::check_response("GET", &url, status, text)?;
        serde_json::from_str(&payload).with_context(|| format!("decoding response from {url}"))
    }

    pub async fn get_text(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.request_raw(reqwest::Method::GET, path).await?;
        Self::check_response("GET", &url, status, text)
    }

    pub async fn post<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.send_body(reqwest::Method::POST, path, body).await?;
        let payload = Self::check_response("POST", &url, status, text)?;
        serde_json::from_str(&payload).with_context(|| format!("decoding response from {url}"))
    }

    pub async fn post_empty(&self, path: &str) -> Result<Json> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.request_raw(reqwest::Method::POST, path).await?;
        let payload = Self::check_response("POST", &url, status, text)?;
        if payload.is_empty() || payload == "null" {
            return Ok(Json::Null);
        }
        Ok(serde_json::from_str(&payload).unwrap_or(Json::String(payload)))
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.request_raw(reqwest::Method::DELETE, path).await?;
        Self::check_response("DELETE", &url, status, text)?;
        Ok(())
    }

    pub async fn put<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.send_body(reqwest::Method::PUT, path, body).await?;
        let payload = Self::check_response("PUT", &url, status, text)?;
        serde_json::from_str(&payload).with_context(|| format!("decoding response from {url}"))
    }

    pub async fn patch_json<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base, path);
        let (status, text) = self.send_body(reqwest::Method::PATCH, path, body).await?;
        let payload = Self::check_response("PATCH", &url, status, text)?;
        serde_json::from_str(&payload).with_context(|| format!("decoding response from {url}"))
    }
}
