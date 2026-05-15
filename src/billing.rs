//! Tiny `reqwest` client for the Lynxx billing API.
//!
//! Distinct from `src/client.rs`, which talks to the local gateway admin
//! API. This module hits the hosted billing service that issues API keys at
//! signup. Default base URL is `https://api.lynxx.vexarr.com`, overridable
//! via `--billing-url` or `LYNXX_BILLING_URL`.
//!
//! Only the endpoints we actually call are modelled. Add more methods as
//! we grow surface area — do not reach for a heavier abstraction.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Client as HttpClient, StatusCode};
use serde::{Deserialize, Serialize};

/// Default base URL for the hosted billing API.
pub const DEFAULT_BILLING_URL: &str = "https://api.lynxx.vexarr.com";

/// Response from `GET /v1/account/me`.
///
/// Fields beyond the documented set are tolerated via `#[serde(default)]`
/// and `Option`, so server-side additions won't break the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResp {
    /// Stable customer identifier (URN). Always present.
    pub customer_urn: String,
    /// Account email address.
    pub email: String,
    /// Subscription tier label (e.g. `"free"`, `"pro"`).
    pub tier: String,
    /// Currently active gateway node count.
    #[serde(default)]
    pub active_nodes: u32,
    /// Hard cap on nodes for the current tier. `None` means unlimited.
    #[serde(default)]
    pub max_nodes: Option<u32>,
}

/// Billing-API client. Cheap to clone (wraps an `Arc` internally via `reqwest::Client`).
#[derive(Debug, Clone)]
pub struct BillingClient {
    base_url: String,
    http: HttpClient,
}

impl BillingClient {
    /// Construct a new client. `base_url` is trimmed of trailing `/` so call
    /// sites can freely use `format!("{base}/v1/...")` without doubling.
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // Reasonable defaults: 30s overall, 10s connect. Customer-facing
        // commands so we err toward responsiveness over patience.
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("lynxx-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client builds (no proxy plumbing)");
        Self { base_url, http }
    }

    /// `GET /v1/account/me` with `Authorization: Bearer <api_key>`.
    ///
    /// Errors:
    /// - 401: invalid/expired key -> human-friendly "invalid API key"
    /// - 5xx: surfaced as a server-side error
    /// - other non-2xx: surfaced with status + body excerpt
    /// - transport / DNS / TLS: bubbled through `anyhow::Context`
    pub async fn me(&self, api_key: &str) -> Result<MeResp> {
        let url = format!("{}/v1/account/me", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(api_key)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            bail!("invalid API key (HTTP 401 from {url})");
        }
        if status.is_server_error() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "billing API server error (HTTP {} from {url}): {}",
                status.as_u16(),
                truncate(&body, 200),
            );
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "billing API request failed (HTTP {} from {url}): {}",
                status.as_u16(),
                truncate(&body, 200),
            );
        }

        resp.json::<MeResp>()
            .await
            .with_context(|| format!("decoding response from {url}"))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
