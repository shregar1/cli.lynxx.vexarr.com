use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Wire types — minimal mirror of the admin API surface.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub retries: u32,
    pub connect_timeout_ms: u64,
    #[serde(default)]
    pub lb_strategy: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: Uuid,
    pub paths: Vec<String>,
    pub methods: Vec<String>,
    pub service_id: Uuid,
    pub strip_path: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstance {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    pub enabled: bool,
    pub config: Json,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consumer {
    pub id: Uuid,
    pub username: String,
    pub custom_id: Option<String>,
    pub api_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: Uuid,
    pub service_id: Uuid,
    pub url: String,
    pub weight: u32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── recording wire types ─────────────────────────────────────────────────
//
// Mirrors of `crate::recording::*` shapes — kept here so the CLI binary
// doesn't drag in the gateway crate.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingIndexRow {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default)]
    pub consumer: Option<String>,
    pub bytes_request: u64,
    pub bytes_response: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub ts: DateTime<Utc>,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default)]
    pub consumer: Option<String>,
    pub request: RecordedRequest,
    pub response: RecordedResponse,
    pub upstream_url: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub original: Recording,
    pub new_response: RecordedResponse,
    pub diff: ResponseDiff,
    pub elapsed_ms: u64,
    pub target_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseDiff {
    #[serde(default)]
    pub status_changed: Option<(u16, u16)>,
    #[serde(default)]
    pub headers_added: Vec<(String, String)>,
    #[serde(default)]
    pub headers_removed: Vec<(String, String)>,
    #[serde(default)]
    pub headers_changed: Vec<(String, String, String)>,
    #[serde(default)]
    pub body_diff: BodyDiff,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyDiff {
    #[default]
    Identical,
    BinaryDiff {
        before_bytes: usize,
        after_bytes: usize,
    },
    JsonDiff {
        added: Json,
        removed: Json,
        changed: Json,
    },
    TextDiff {
        lines_changed: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: i64,
    pub services_count: usize,
    pub routes_count: usize,
    pub plugins_count: usize,
    pub consumers_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: Uuid,
    #[serde(default)]
    pub urn: String,
    pub ts: DateTime<Utc>,
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub route_id: Option<Uuid>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub status: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub client_ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTestResult {
    pub url: String,
    pub status: u16,
    pub latency_ms: u64,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body_preview: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTestResponse {
    pub external: RouteTestResult,
    pub internal: RouteTestResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployResponse {
    pub deploy_id: Uuid,
    pub service_id: Uuid,
    pub new_target_id: Uuid,
    pub state: String,
    pub health_checks_required: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStatusResponse {
    pub service_id: Uuid,
    pub state: String,
    pub new_target_id: Uuid,
    #[serde(default)]
    pub old_target_ids: Vec<Uuid>,
    pub health_checks_passed: u32,
    pub health_checks_required: u32,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service_id: Uuid,
    pub status: String,
    pub circuit: String,
    pub consecutive_failures: u32,
    pub last_check_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminTokenView {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: DateTime<Utc>,
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub actor_name: Option<String>,
    #[serde(default)]
    pub actor_role: Option<String>,
    pub action: String,
    pub resource_type: String,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub diff: Option<Json>,
    pub hash: String,
    #[serde(default)]
    pub prev_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditVerifyResult {
    pub ok: bool,
    pub checked: u64,
    #[serde(default)]
    pub broken_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeStatusView {
    pub enabled: bool,
    #[serde(default)]
    pub has_cert: Option<bool>,
    #[serde(default)]
    pub http01_challenges: Option<usize>,
    #[serde(default)]
    pub tls_alpn01_challenges: Option<usize>,
}

// ---------------------------------------------------------------------------
// Declarative schema (dump / apply)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeclarativeState {
    #[serde(default)]
    pub services: Vec<DeclService>,
    #[serde(default)]
    pub routes: Vec<DeclRoute>,
    #[serde(default)]
    pub plugins: Vec<DeclPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclService {
    pub name: String,
    pub url: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_lb")]
    pub lb_strategy: String,
    #[serde(default)]
    pub targets: Vec<DeclTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclTarget {
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclRoute {
    pub service: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub strip_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclPlugin {
    pub name: String,
    /// One of: "global", or attached to a named service / route.
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub service: Option<String>,
    /// Route reference: paths joined by ";" — must match an existing route's paths set.
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub config: Json,
}

pub fn default_retries() -> u32 {
    3
}
pub fn default_timeout() -> u64 {
    5000
}
pub fn default_weight() -> u32 {
    1
}
pub fn default_lb() -> String {
    "round-robin".into()
}
pub fn default_scope() -> String {
    "global".into()
}

/// Inline catalog for `validate`. Mirrors the gateway's built-in plugins.
pub const PLUGIN_CATALOG: &[&str] = &[
    "jwt",
    "key-auth",
    "basic-auth",
    "hmac-auth",
    "rate-limit",
    "rate-limiting",
    "cors",
    "ip-restriction",
    "request-transformer",
    "response-transformer",
    "request-size-limit",
    "prometheus",
    "logging",
    "correlation-id",
    "acl",
    "oauth2",
];
