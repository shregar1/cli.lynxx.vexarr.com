//! lynxx — declarative CLI for the Lynxx gateway admin API.
//!
//! Talks HTTP to the gateway's admin port (default `http://localhost:9001`).
//! Override via `LYNXX_GATEWAY_BASE_URL` env var or `--gateway-url` flag.
//! Produces human-friendly tables by default and JSON/YAML on request, plus
//! an `apply`/`dump` pair for idempotent declarative state management,
//! a live `watch`/`tail`, and a `exec` smoke-tester.
//!
//! Aesthetic: monochrome ANSI (bold + dim only) plus light box-drawing
//! borders on TTY tables. Errors go to stderr with a "✗" prefix and a
//! dim "└─" continuation when stdout is a TTY.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use serde_json::Value as Json;
use uuid::Uuid;

mod cli;
mod client;
mod commands;
mod config;
mod models;
mod openapi;
mod output;
mod resolve;
mod util;

// Re-export at crate root so sibling modules can use `crate::OutputFormat` etc.
pub(crate) use cli::*;
pub(crate) use client::Client;
pub(crate) use models::*;
pub(crate) use output::*;

use commands::{
    admin::*,
    declarative::{apply_state, cmd_apply, cmd_dump, cmd_validate},
    exec::{cmd_exec, cmd_start},
    live::{cmd_tail, cmd_watch},
    plugins::*,
    record::*,
    resources::*,
    routes::*,
    services::*,
};
use config::{cmd_config, load_config};
use openapi::openapi_to_state;
use resolve::resolve_service;
use util::{humanize_secs, print_error};

// (CLI definition is in src/cli.rs, re-exported via `pub(crate) use cli::*;`)

// (All CLI enum definitions live in src/cli.rs.
//  The `pub(crate) use cli::*` above brings them into scope.)

// ---------------------------------------------------------------------------
// Wire types, Client, output helpers, resolve helpers — now in dedicated
// modules under src/.  The block that used to live here has been deleted;
// see models.rs, client.rs, output.rs, resolve.rs.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Commands not yet extracted to src/commands/.
// ---------------------------------------------------------------------------

async fn cmd_ping(client: &Client) -> Result<()> {
    let s: StatusResponse = client.get("/status").await?;
    println!(
        "{} lynxx {} — {} (up {})",
        green("✓"),
        s.version,
        s.status,
        humanize_secs(s.uptime_seconds)
    );
    println!(
        "  {} services · {} routes · {} plugins · {} consumers",
        s.services_count, s.routes_count, s.plugins_count, s.consumers_count,
    );
    Ok(())
}

async fn cmd_status(client: &Client, fmt: OutputFormat) -> Result<()> {
    let s: StatusResponse = client.get("/status").await?;
    emit(&s, fmt)
}


async fn cmd_import(
    client: &Client,
    spec_src: String,
    service_prefix: Option<String>,
    upstream_override: Option<String>,
    strip_base_path: bool,
    validators: bool,
    mock: bool,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    let raw = if spec_src.starts_with("http://") || spec_src.starts_with("https://") {
        let resp = reqwest::Client::new()
            .get(&spec_src)
            .send()
            .await
            .with_context(|| format!("GET {spec_src}"))?;
        if !resp.status().is_success() {
            bail!("GET {spec_src} → HTTP {}", resp.status());
        }
        resp.text().await.context("reading spec body")?
    } else {
        fs::read_to_string(&spec_src).with_context(|| format!("reading {spec_src}"))?
    };

    let spec: openapiv3::OpenAPI = match serde_json::from_str::<openapiv3::OpenAPI>(&raw) {
        Ok(s) => s,
        Err(_) => serde_yaml::from_str::<openapiv3::OpenAPI>(&raw)
            .context("parsing OpenAPI spec (tried JSON then YAML)")?,
    };

    let state = openapi_to_state(
        &spec,
        &spec_src,
        service_prefix.as_deref(),
        upstream_override.as_deref(),
        strip_base_path,
        validators,
        mock,
    )?;

    if state.services.is_empty() {
        eprintln!(
            "{} spec produced no services — check `servers[0].url` or pass --upstream",
            yellow("!")
        );
        return Ok(());
    }

    apply_state(client, state, dry_run, auto_approve).await
}

async fn cmd_analytics_traffic(
    client: &Client,
    service: &str,
    range: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/analytics?range={}", svc.id, range))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|b| {
                vec![
                    b.get("timestamp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    b.get("requests")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    b.get("errors")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    b.get("avg_latency_ms")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{:.1}", v))
                        .unwrap_or_default(),
                    b.get("p99_latency_ms")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{:.1}", v))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["TIMESTAMP", "REQUESTS", "ERRORS", "AVG_MS", "P99_MS"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_analytics_rate_limits(
    client: &Client,
    service: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/rate-limits", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|b| {
                vec![
                    b.get("key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    b.get("allowed")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    b.get("blocked")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    b.get("remaining")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["KEY", "ALLOWED", "BLOCKED", "REMAINING"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_analytics_rate_limit_stats(
    client: &Client,
    service: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/rate-limit-stats", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

async fn cmd_analytics_geo(
    client: &Client,
    service: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/geo", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|b| {
                vec![
                    b.get("country")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    b.get("requests")
                        .and_then(|v| v.as_u64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    b.get("avg_latency_ms")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{:.1}", v))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["COUNTRY", "REQUESTS", "AVG_MS"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

async fn cmd_probes_list(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/probes", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|p| {
                vec![
                    p.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    p.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    p.get("method")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    p.get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    p.get("enabled")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "yes" } else { "no" })
                        .unwrap_or("-")
                        .to_string(),
                    p.get("interval_seconds")
                        .and_then(|v| v.as_i64())
                        .map(|v| format!("{}s", v))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "METHOD", "PATH", "ENABLED", "INTERVAL"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_probes_create(
    client: &Client,
    service: &str,
    name: String,
    route_id: Option<Uuid>,
    method: String,
    path: String,
    interval: i32,
    timeout: i32,
    expected_status: i32,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let body = serde_json::json!({
        "name": name,
        "route_id": route_id,
        "method": method,
        "path": path,
        "interval_seconds": interval,
        "timeout_ms": timeout,
        "expected_status": expected_status,
    });
    let result: Json = client
        .post(&format!("/services/{}/probes", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} created probe {} ({})", green("✓"), name, id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_probes_patch(
    client: &Client,
    id: &str,
    enabled: Option<bool>,
    name: Option<String>,
    method: Option<String>,
    path: Option<String>,
    interval: Option<i32>,
    timeout: Option<i32>,
    expected_status: Option<i32>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    if let Some(v) = enabled {
        body.insert("enabled".into(), Json::Bool(v));
    }
    if let Some(v) = name {
        body.insert("name".into(), Json::String(v));
    }
    if let Some(v) = method {
        body.insert("method".into(), Json::String(v));
    }
    if let Some(v) = path {
        body.insert("path".into(), Json::String(v));
    }
    if let Some(v) = interval {
        body.insert("interval_seconds".into(), serde_json::json!(v));
    }
    if let Some(v) = timeout {
        body.insert("timeout_ms".into(), serde_json::json!(v));
    }
    if let Some(v) = expected_status {
        body.insert("expected_status".into(), serde_json::json!(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let result: Json = client
        .patch_json(&format!("/probes/{id}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} updated probe {}", green("✓"), id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_probes_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/probes/{id}")).await?;
    println!("{} deleted probe {}", green("✓"), id);
    Ok(())
}

async fn cmd_probes_results(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let data: Json = client.get(&format!("/probes/{id}/results")).await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|r| {
                vec![
                    r.get("checked_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    r.get("status_code")
                        .and_then(|v| v.as_i64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    r.get("latency_ms")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{:.1}", v))
                        .unwrap_or_default(),
                    r.get("success")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "ok" } else { "fail" })
                        .unwrap_or("-")
                        .to_string(),
                    r.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["CHECKED_AT", "STATUS", "LATENCY_MS", "RESULT", "ERROR"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_probes_stats(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let data: Json = client.get(&format!("/probes/{id}/stats")).await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

async fn cmd_snapshots_list(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/snapshots", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|s| {
                vec![
                    s.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    s.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    s.get("created_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "CREATED_AT"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_snapshots_create(
    client: &Client,
    service: &str,
    name: String,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let body = serde_json::json!({ "name": name });
    let result: Json = client
        .post(&format!("/services/{}/snapshots", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} created snapshot {} ({})", green("✓"), name, id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_snapshots_diff(
    client: &Client,
    service: &str,
    snapshot_id: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/snapshots/{}/diff", svc.id, snapshot_id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    // Diff data is typically structured JSON — pretty-print in table mode too.
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

async fn cmd_snapshots_rollback(
    client: &Client,
    service: &str,
    snapshot_id: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let result: Json = client
        .post_empty(&format!(
            "/services/{}/snapshots/{}/rollback",
            svc.id, snapshot_id
        ))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} rolled back to snapshot {}", green("✓"), snapshot_id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_snapshots_delete(client: &Client, service: &str, snapshot_id: &str) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    client
        .delete(&format!(
            "/services/{}/snapshots/{}",
            svc.id, snapshot_id
        ))
        .await?;
    println!("{} deleted snapshot {}", green("✓"), snapshot_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

async fn cmd_alerts_list(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/alerts", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|a| {
                vec![
                    a.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    a.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    a.get("metric")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    a.get("operator")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    a.get("threshold")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{}", v))
                        .unwrap_or_default(),
                    a.get("enabled")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "yes" } else { "no" })
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "METRIC", "OPERATOR", "THRESHOLD", "ENABLED"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_alerts_create(
    client: &Client,
    service: &str,
    name: String,
    metric: String,
    operator: String,
    threshold: f64,
    window: i32,
    cooldown: i32,
    channels: String,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let channels_json: Json =
        serde_json::from_str(&channels).context("--channels must be valid JSON")?;
    let body = serde_json::json!({
        "name": name,
        "metric": metric,
        "operator": operator,
        "threshold": threshold,
        "window_seconds": window,
        "cooldown_seconds": cooldown,
        "channels": channels_json,
    });
    let result: Json = client
        .post(&format!("/services/{}/alerts", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} created alert rule {} ({})", green("✓"), name, id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_alerts_patch(
    client: &Client,
    id: &str,
    enabled: Option<bool>,
    name: Option<String>,
    threshold: Option<f64>,
    window: Option<i32>,
    cooldown: Option<i32>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    if let Some(v) = enabled {
        body.insert("enabled".into(), Json::Bool(v));
    }
    if let Some(v) = name {
        body.insert("name".into(), Json::String(v));
    }
    if let Some(v) = threshold {
        body.insert("threshold".into(), serde_json::json!(v));
    }
    if let Some(v) = window {
        body.insert("window_seconds".into(), serde_json::json!(v));
    }
    if let Some(v) = cooldown {
        body.insert("cooldown_seconds".into(), serde_json::json!(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let result: Json = client
        .patch_json(&format!("/alerts/{id}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} updated alert rule {}", green("✓"), id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_alerts_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/alerts/{id}")).await?;
    println!("{} deleted alert rule {}", green("✓"), id);
    Ok(())
}

async fn cmd_alerts_events(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/alert-events", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|e| {
                vec![
                    e.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("rule_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("metric")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("value")
                        .and_then(|v| v.as_f64())
                        .map(|v| format!("{}", v))
                        .unwrap_or_default(),
                    e.get("fired_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("resolved")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "yes" } else { "no" })
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["ID", "RULE", "METRIC", "VALUE", "FIRED_AT", "RESOLVED"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_alerts_resolve(client: &Client, id: &str) -> Result<()> {
    let _: Json = client
        .post_empty(&format!("/alert-events/{id}/resolve"))
        .await?;
    println!("{} resolved alert event {}", green("✓"), id);
    Ok(())
}

async fn cmd_alerts_test(client: &Client, id: &str) -> Result<()> {
    let _: Json = client
        .post_empty(&format!("/alerts/{id}/test"))
        .await?;
    println!("{} fired test alert for rule {}", green("✓"), id);
    Ok(())
}

// ---------------------------------------------------------------------------
// IP Rules
// ---------------------------------------------------------------------------

async fn cmd_ip_rules_list(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/ip-rules", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|r| {
                vec![
                    r.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    r.get("cidr")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    r.get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    r.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    r.get("priority")
                        .and_then(|v| v.as_i64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    r.get("enabled")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "yes" } else { "no" })
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["ID", "CIDR", "ACTION", "DESCRIPTION", "PRIORITY", "ENABLED"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_ip_rules_create(
    client: &Client,
    service: &str,
    cidr: String,
    action: String,
    description: Option<String>,
    route_id: Option<Uuid>,
    priority: i32,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let body = serde_json::json!({
        "cidr": cidr,
        "action": action,
        "description": description,
        "route_id": route_id,
        "priority": priority,
    });
    let result: Json = client
        .post(&format!("/services/{}/ip-rules", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} created IP rule {} ({})", green("✓"), cidr, id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_ip_rules_patch(
    client: &Client,
    id: &str,
    enabled: Option<bool>,
    cidr: Option<String>,
    action: Option<String>,
    description: Option<String>,
    priority: Option<i32>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    if let Some(v) = enabled {
        body.insert("enabled".into(), Json::Bool(v));
    }
    if let Some(v) = cidr {
        body.insert("cidr".into(), Json::String(v));
    }
    if let Some(v) = action {
        body.insert("action".into(), Json::String(v));
    }
    if let Some(v) = description {
        body.insert("description".into(), Json::String(v));
    }
    if let Some(v) = priority {
        body.insert("priority".into(), serde_json::json!(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let result: Json = client
        .patch_json(&format!("/ip-rules/{id}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} updated IP rule {}", green("✓"), id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_ip_rules_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/ip-rules/{id}")).await?;
    println!("{} deleted IP rule {}", green("✓"), id);
    Ok(())
}

async fn cmd_ip_rules_bulk(
    client: &Client,
    service: &str,
    file: &std::path::Path,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let content = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read {}", file.display()))?;
    let rules: Json = serde_json::from_str(&content)
        .with_context(|| format!("invalid JSON in {}", file.display()))?;
    let body = serde_json::json!({ "rules": rules });
    let result: Json = client
        .post(&format!("/services/{}/ip-rules/bulk", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let count = result.as_array().map(|a| a.len()).unwrap_or(0);
        println!("{} bulk-created {} IP rules", green("✓"), count);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error Pages
// ---------------------------------------------------------------------------

async fn cmd_error_pages_list(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/error-pages", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|e| {
                vec![
                    e.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("status_code")
                        .and_then(|v| v.as_i64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    e.get("content_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    e.get("enabled")
                        .and_then(|v| v.as_bool())
                        .map(|v| if v { "yes" } else { "no" })
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["ID", "STATUS_CODE", "CONTENT_TYPE", "ENABLED"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_error_pages_create(
    client: &Client,
    service: &str,
    status_code: i32,
    content_type: String,
    body_content: String,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let actual_body = if let Some(path) = body_content.strip_prefix('@') {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path))?
    } else {
        body_content
    };
    let body = serde_json::json!({
        "status_code": status_code,
        "content_type": content_type,
        "body": actual_body,
    });
    let result: Json = client
        .post(&format!("/services/{}/error-pages", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!(
            "{} created error page for status {} ({})",
            green("✓"),
            status_code,
            id
        );
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_error_pages_patch(
    client: &Client,
    id: &str,
    content_type: Option<String>,
    body_content: Option<String>,
    enabled: Option<bool>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    if let Some(v) = content_type {
        body.insert("content_type".into(), Json::String(v));
    }
    if let Some(v) = body_content {
        let actual = if let Some(path) = v.strip_prefix('@') {
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path))?
        } else {
            v
        };
        body.insert("body".into(), Json::String(actual));
    }
    if let Some(v) = enabled {
        body.insert("enabled".into(), Json::Bool(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let result: Json = client
        .patch_json(&format!("/error-pages/{id}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} updated error page {}", green("✓"), id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_error_pages_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/error-pages/{id}")).await?;
    println!("{} deleted error page {}", green("✓"), id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Consumer Groups
// ---------------------------------------------------------------------------

async fn cmd_consumer_groups_list(
    client: &Client,
    service: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/consumer-groups", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|g| {
                vec![
                    g.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    g.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    g.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    g.get("rate_limit")
                        .and_then(|v| v.as_i64())
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    g.get("rate_window")
                        .and_then(|v| v.as_i64())
                        .map(|v| format!("{}s", v))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        print_table(&["ID", "NAME", "DESCRIPTION", "RATE_LIMIT", "WINDOW"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_consumer_groups_create(
    client: &Client,
    service: &str,
    name: String,
    description: Option<String>,
    rate_limit: Option<i32>,
    rate_window: Option<i32>,
    burst_limit: Option<i32>,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let body = serde_json::json!({
        "name": name,
        "description": description,
        "rate_limit": rate_limit,
        "rate_window": rate_window,
        "burst_limit": burst_limit,
    });
    let result: Json = client
        .post(&format!("/services/{}/consumer-groups", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        println!("{} created consumer group {} ({})", green("✓"), name, id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_consumer_groups_patch(
    client: &Client,
    id: &str,
    name: Option<String>,
    description: Option<String>,
    rate_limit: Option<i32>,
    rate_window: Option<i32>,
    burst_limit: Option<i32>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    if let Some(v) = name {
        body.insert("name".into(), Json::String(v));
    }
    if let Some(v) = description {
        body.insert("description".into(), Json::String(v));
    }
    if let Some(v) = rate_limit {
        body.insert("rate_limit".into(), serde_json::json!(v));
    }
    if let Some(v) = rate_window {
        body.insert("rate_window".into(), serde_json::json!(v));
    }
    if let Some(v) = burst_limit {
        body.insert("burst_limit".into(), serde_json::json!(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let result: Json = client
        .patch_json(&format!("/consumer-groups/{id}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!("{} updated consumer group {}", green("✓"), id);
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_consumer_groups_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/consumer-groups/{id}")).await?;
    println!("{} deleted consumer group {}", green("✓"), id);
    Ok(())
}

async fn cmd_consumer_groups_members(
    client: &Client,
    id: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let data: Json = client
        .get(&format!("/consumer-groups/{id}/members"))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    if let Some(arr) = data.as_array() {
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|m| {
                vec![
                    m.get("consumer_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    m.get("username")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    m.get("added_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                ]
            })
            .collect();
        print_table(&["CONSUMER_ID", "USERNAME", "ADDED_AT"], &rows);
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }
    Ok(())
}

async fn cmd_consumer_groups_add_member(
    client: &Client,
    id: &str,
    consumer_id: Uuid,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "consumer_id": consumer_id });
    let result: Json = client
        .post(&format!("/consumer-groups/{id}/members"), &body)
        .await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} added consumer {} to group {}",
            green("✓"),
            consumer_id,
            id
        );
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

async fn cmd_consumer_groups_remove_member(
    client: &Client,
    id: &str,
    consumer_id: &str,
) -> Result<()> {
    client
        .delete(&format!("/consumer-groups/{id}/members/{consumer_id}"))
        .await?;
    println!(
        "{} removed consumer {} from group {}",
        green("✓"),
        consumer_id,
        id
    );
    Ok(())
}

async fn cmd_consumer_groups_usage(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let data: Json = client
        .get(&format!("/consumer-groups/{id}/usage"))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

async fn cmd_graph(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/graph", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    // Dependency graph is structured JSON — pretty-print in table mode.
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------

async fn cmd_docs_show(client: &Client, service: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    let data: Json = client
        .get(&format!("/services/{}/docs", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    // API docs are structured JSON — pretty-print in table mode.
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

async fn cmd_docs_try(
    client: &Client,
    method: String,
    url: String,
    headers: Vec<String>,
    body: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut hdrs = HashMap::new();
    for h in &headers {
        if let Some((k, v)) = h.split_once(':') {
            hdrs.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let req_body = serde_json::json!({
        "method": method,
        "url": url,
        "headers": hdrs,
        "body": body,
    });
    let result: Json = client.post("/proxy/try", &req_body).await?;
    if fmt == OutputFormat::Table {
        let status = result
            .get("status")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let resp_body = result
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("{} {} {}", green("✓"), method, url);
        println!("Status: {}", status);
        if !resp_body.is_empty() {
            println!("{}", resp_body);
        }
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Proxy Replay
// ---------------------------------------------------------------------------

async fn cmd_proxy_replay(
    client: &Client,
    method: String,
    url: String,
    headers: Vec<String>,
    body: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut hdrs = HashMap::new();
    for h in &headers {
        if let Some((k, v)) = h.split_once(':') {
            hdrs.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let req_body = serde_json::json!({
        "method": method,
        "url": url,
        "headers": hdrs,
        "body": body,
    });
    let result: Json = client.post("/proxy/replay", &req_body).await?;
    if fmt == OutputFormat::Table {
        let status = result
            .get("status")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let resp_body = result
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("{} {} {}", green("✓"), method, url);
        println!("Status: {}", status);
        if !resp_body.is_empty() {
            println!("{}", resp_body);
        }
    } else {
        emit(&result, fmt)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------


#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let local = load_config();

    // Color resolution: --no-color > NO_COLOR > config file > tty check.
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    if cli.no_color || no_color_env || local.no_color.unwrap_or(false) {
        COLOR_OFF.store(true, Ordering::Relaxed);
    }
    if cli.full_id || local.full_id.unwrap_or(false) {
        FULL_ID.store(true, Ordering::Relaxed);
    }

    // Resolve gateway URL: --gateway-url > LYNXX_GATEWAY_BASE_URL > config file > default.
    let gateway_url = cli
        .gateway_url
        .clone()
        .or_else(|| local.gateway_url.clone())
        .unwrap_or_else(|| "http://localhost:9001".into());

    // Resolve API key: --api-key > LYNXX_API_KEY > config file.
    let api_key = cli.api_key.clone().or_else(|| local.api_key.clone());

    // Resolve Lynxx platform: base URL, license key, secret key.
    let _base_url = cli
        .base_url
        .clone()
        .or_else(|| local.base_url.clone());
    let _license_key = cli
        .license_key
        .clone()
        .or_else(|| local.license_key.clone());
    let _secret_key = cli
        .secret_key
        .clone()
        .or_else(|| local.secret_key.clone());

    // Resolve telemetry: --telemetry-url > LYNXX_TELEMETRY_URL > config file.
    let _telemetry_url = cli
        .telemetry_url
        .clone()
        .or_else(|| local.telemetry_url.clone());
    let _telemetry_api_key = cli
        .telemetry_api_key
        .clone()
        .or_else(|| local.telemetry_api_key.clone());

    // Resolve output format: --output > config file > Table.
    let output = cli.output.unwrap_or_else(|| match local.output.as_deref() {
        Some("json") => OutputFormat::Json,
        Some("yaml") => OutputFormat::Yaml,
        _ => OutputFormat::Table,
    });

    let client = Client::new(&gateway_url, api_key);

    let result: Result<()> = match cli.command {
        Cmd::Ping => cmd_ping(&client).await,
        Cmd::Status => cmd_status(&client, output).await,
        Cmd::Projects(p) => match p {
            ProjectsCmd::List => cmd_projects_list(&client, output).await,
            ProjectsCmd::Get { ident } => cmd_projects_get(&client, &ident, output).await,
            ProjectsCmd::Create { name, description } => {
                cmd_projects_create(&client, name, description, output).await
            }
            ProjectsCmd::Update {
                ident,
                name,
                description,
            } => cmd_projects_update(&client, &ident, name, description, output).await,
            ProjectsCmd::Delete { ident } => cmd_projects_delete(&client, &ident).await,
            ProjectsCmd::Services { ident } => {
                cmd_projects_services(&client, &ident, output).await
            }
        },
        Cmd::Services(s) => match s {
            ServicesCmd::List => cmd_services_list(&client, output).await,
            ServicesCmd::Get { ident } => cmd_services_get(&client, &ident, output).await,
            ServicesCmd::Create {
                name,
                url,
                retries,
                connect_timeout_ms,
                lb_strategy,
                domain,
                subdomain,
                project_id,
            } => {
                cmd_services_create(
                    &client,
                    name,
                    url,
                    retries,
                    connect_timeout_ms,
                    lb_strategy,
                    domain,
                    subdomain,
                    project_id,
                    output,
                )
                .await
            }
            ServicesCmd::Update {
                ident,
                name,
                url,
                retries,
                connect_timeout_ms,
                lb_strategy,
                domain,
                subdomain,
            } => {
                cmd_services_update(
                    &client,
                    &ident,
                    name,
                    url,
                    retries,
                    connect_timeout_ms,
                    lb_strategy,
                    domain,
                    subdomain,
                    output,
                )
                .await
            }
            ServicesCmd::Patch {
                ident,
                name,
                url,
                retries,
                connect_timeout_ms,
                lb_strategy,
                domain,
                subdomain,
                project_id,
            } => {
                cmd_services_patch(
                    &client,
                    &ident,
                    name,
                    url,
                    retries,
                    connect_timeout_ms,
                    lb_strategy,
                    domain,
                    subdomain,
                    project_id,
                    output,
                )
                .await
            }
            ServicesCmd::Delete { ident } => cmd_services_delete(&client, &ident).await,
        },
        Cmd::Routes(r) => match r {
            RoutesCmd::List => cmd_routes_list(&client, output).await,
            RoutesCmd::Get { id } => cmd_routes_get(&client, &id, output).await,
            RoutesCmd::Create {
                service,
                paths,
                methods,
                strip_path,
            } => cmd_routes_create(&client, service, paths, methods, strip_path, output).await,
            RoutesCmd::Update {
                id,
                service,
                paths,
                methods,
                strip_path,
            } => cmd_routes_update(&client, &id, service, paths, methods, strip_path, output).await,
            RoutesCmd::Patch {
                id,
                service,
                paths,
                methods,
                strip_path,
            } => cmd_routes_patch(&client, &id, service, paths, methods, strip_path, output).await,
            RoutesCmd::Delete { id } => cmd_routes_delete(&client, &id).await,
            RoutesCmd::Test {
                id,
                method,
                path,
                headers,
            } => cmd_routes_test(&client, &id, &method, &path, &headers, output).await,
        },
        Cmd::Plugins(p) => match p {
            PluginsCmd::List => cmd_plugins_list(&client, output).await,
            PluginsCmd::Install {
                name,
                service,
                route,
                config,
            } => cmd_plugins_install(&client, name, service, route, config, output).await,
            PluginsCmd::Delete { id } => cmd_plugins_delete(&client, &id).await,
        },
        Cmd::Wasm(w) => match w {
            WasmCmd::Install {
                path,
                name,
                config,
                service,
                route,
                fuel,
                memory_kb,
                wall_ms,
            } => {
                cmd_wasm_install(
                    &client, path, name, config, service, route, fuel, memory_kb, wall_ms, output,
                )
                .await
            }
            WasmCmd::List => cmd_wasm_list(&client, output).await,
            WasmCmd::Remove { id } => cmd_wasm_remove(&client, &id).await,
        },
        Cmd::Scripts(s) => match s {
            ScriptsCmd::Install {
                on_request_cmd,
                on_request_args,
                on_response_cmd,
                on_response_args,
                timeout,
                env,
                max_body_bytes,
                service,
                route,
            } => {
                cmd_scripts_install(
                    &client,
                    on_request_cmd,
                    on_request_args,
                    on_response_cmd,
                    on_response_args,
                    timeout,
                    env,
                    max_body_bytes,
                    service,
                    route,
                    output,
                )
                .await
            }
            ScriptsCmd::List => cmd_scripts_list(&client, output).await,
            ScriptsCmd::Delete { id } => cmd_scripts_delete(&client, &id).await,
        },
        Cmd::Consumers(c) => match c {
            ConsumersCmd::List => cmd_consumers_list(&client, output).await,
            ConsumersCmd::Create {
                username,
                custom_id,
            } => cmd_consumers_create(&client, username, custom_id, output).await,
            ConsumersCmd::Delete { ident } => cmd_consumers_delete(&client, &ident).await,
        },
        Cmd::Targets(t) => match t {
            TargetsCmd::List { service } => cmd_targets_list(&client, service, output).await,
            TargetsCmd::Add {
                service,
                url,
                weight,
            } => cmd_targets_add(&client, service, url, weight, output).await,
            TargetsCmd::Delete { id } => cmd_targets_delete(&client, &id).await,
        },
        Cmd::Tokens(t) => match t {
            TokensCmd::List => cmd_tokens_list(&client, output).await,
            TokensCmd::Create {
                name,
                role,
                expires_in_days,
            } => cmd_tokens_create(&client, name, role, expires_in_days, output).await,
            TokensCmd::Update { ident, name, role } => {
                cmd_tokens_update(&client, &ident, name, role, output).await
            }
            TokensCmd::Delete { ident } => cmd_tokens_delete(&client, &ident).await,
        },
        Cmd::Audit(a) => match a {
            AuditCmd::List {
                action,
                resource_type,
                limit,
            } => cmd_audit_list(&client, action, resource_type, limit, output).await,
            AuditCmd::Verify => cmd_audit_verify(&client, output).await,
        },
        Cmd::CircuitBreakers(cb) => match cb {
            CircuitBreakersCmd::List => cmd_circuit_breakers_list(&client, output).await,
            CircuitBreakersCmd::Reset { service } => {
                cmd_circuit_breakers_reset(&client, &service).await
            }
        },
        Cmd::Logs(l) => match l {
            LogsCmd::List {
                service_id,
                route_id,
                project_id,
                level,
                search,
                limit,
            } => {
                cmd_logs_list(
                    &client, service_id, route_id, project_id, level, search, limit, output,
                )
                .await
            }
        },
        Cmd::Deploy(d) => match d {
            DeployCmd::Start {
                service,
                target_url,
                health_path,
                health_interval_ms,
                health_timeout_ms,
                health_threshold,
                drain_timeout_ms,
                weight,
            } => {
                cmd_deploy_start(
                    &client,
                    &service,
                    target_url,
                    health_path,
                    health_interval_ms,
                    health_timeout_ms,
                    health_threshold,
                    drain_timeout_ms,
                    weight,
                    output,
                )
                .await
            }
            DeployCmd::Status { service } => {
                cmd_deploy_status(&client, &service, output).await
            }
        },
        Cmd::Quota => cmd_quota(&client, output).await,
        Cmd::Upstreams => cmd_upstreams(&client, output).await,
        Cmd::Metrics => cmd_metrics(&client).await,
        Cmd::Acme(a) => match a {
            AcmeCmd::Status => cmd_acme_status(&client, output).await,
            AcmeCmd::Renew => cmd_acme_renew(&client).await,
            AcmeCmd::Revoke => cmd_acme_revoke(&client).await,
            AcmeCmd::SetHttp01 {
                token,
                key_authorization,
            } => cmd_acme_set_http01(&client, token, key_authorization).await,
            AcmeCmd::ClearHttp01 { token } => cmd_acme_clear_http01(&client, &token).await,
            AcmeCmd::SetTlsAlpn01 {
                domain,
                key_authorization,
            } => cmd_acme_set_tls_alpn01(&client, domain, key_authorization).await,
            AcmeCmd::ClearTlsAlpn01 { domain } => {
                cmd_acme_clear_tls_alpn01(&client, &domain).await
            }
        },
        Cmd::Debug(d) => {
            let endpoint = match d {
                DebugCmd::Heap => "heap",
                DebugCmd::Tasks => "tasks",
                DebugCmd::Threads => "threads",
                DebugCmd::Uptime => "uptime",
                DebugCmd::Stats => "stats",
            };
            cmd_debug(&client, endpoint, output).await
        }
        Cmd::LiveConfig(lc) => match lc {
            LiveConfigCmd::Get => cmd_live_config_get(&client, output).await,
            LiveConfigCmd::Set { path, value } => {
                cmd_live_config_set(&client, &path, &value, output).await
            }
            LiveConfigCmd::Validate { body } => {
                cmd_live_config_validate(&client, &body, output).await
            }
            LiveConfigCmd::History => cmd_live_config_history(&client, output).await,
            LiveConfigCmd::Lookup { tag } => {
                cmd_live_config_lookup(&client, &tag, output).await
            }
        },
        Cmd::Health => cmd_health(&client, output).await,
        Cmd::Reload => cmd_reload(&client).await,
        Cmd::Dump => cmd_dump(&client).await,
        Cmd::Apply {
            file,
            dry_run,
            auto_approve,
        } => cmd_apply(&client, file, dry_run, auto_approve).await,
        Cmd::Import {
            spec,
            service_prefix,
            upstream,
            strip_base_path,
            validators,
            mock,
            dry_run,
            auto_approve,
        } => {
            cmd_import(
                &client,
                spec,
                service_prefix,
                upstream,
                strip_base_path,
                validators,
                mock,
                dry_run,
                auto_approve,
            )
            .await
        }
        Cmd::Validate { file } => cmd_validate(&client, file).await,
        Cmd::Watch { resource, interval } => cmd_watch(&client, resource, interval, output).await,
        Cmd::Tail { interval } => cmd_tail(&client, interval).await,
        Cmd::Exec {
            path,
            method,
            body,
            headers,
            proxy,
            max_body,
        } => cmd_exec(&gateway_url, path, method, body, headers, proxy, max_body).await,
        Cmd::Record(r) => match r {
            RecordCmd::List {
                since,
                method,
                path_prefix,
                status,
                limit,
            } => cmd_record_list(&client, since, method, path_prefix, status, limit, output).await,
            RecordCmd::Show { id } => cmd_record_show(&client, &id, output).await,
            RecordCmd::Replay { id, against } => {
                cmd_record_replay(&client, &id, against, output).await
            }
            RecordCmd::Delete { id } => cmd_record_delete(&client, &id).await,
        },
        Cmd::Analytics(a) => match a {
            AnalyticsCmd::Traffic { service, range } => {
                cmd_analytics_traffic(&client, &service, &range, output).await
            }
            AnalyticsCmd::RateLimits { service } => {
                cmd_analytics_rate_limits(&client, &service, output).await
            }
            AnalyticsCmd::RateLimitStats { service } => {
                cmd_analytics_rate_limit_stats(&client, &service, output).await
            }
            AnalyticsCmd::Geo { service } => {
                cmd_analytics_geo(&client, &service, output).await
            }
        },
        Cmd::Probes(p) => match p {
            ProbesCmd::List { service } => {
                cmd_probes_list(&client, &service, output).await
            }
            ProbesCmd::Create {
                service,
                name,
                route_id,
                method,
                path,
                interval,
                timeout,
                expected_status,
            } => {
                cmd_probes_create(
                    &client,
                    &service,
                    name,
                    route_id,
                    method,
                    path,
                    interval,
                    timeout,
                    expected_status,
                    output,
                )
                .await
            }
            ProbesCmd::Patch {
                id,
                enabled,
                name,
                method,
                path,
                interval,
                timeout,
                expected_status,
            } => {
                cmd_probes_patch(
                    &client,
                    &id,
                    enabled,
                    name,
                    method,
                    path,
                    interval,
                    timeout,
                    expected_status,
                    output,
                )
                .await
            }
            ProbesCmd::Delete { id } => cmd_probes_delete(&client, &id).await,
            ProbesCmd::Results { id } => {
                cmd_probes_results(&client, &id, output).await
            }
            ProbesCmd::Stats { id } => {
                cmd_probes_stats(&client, &id, output).await
            }
        },
        Cmd::Snapshots(s) => match s {
            SnapshotsCmd::List { service } => {
                cmd_snapshots_list(&client, &service, output).await
            }
            SnapshotsCmd::Create { service, name } => {
                cmd_snapshots_create(&client, &service, name, output).await
            }
            SnapshotsCmd::Diff {
                service,
                snapshot_id,
            } => cmd_snapshots_diff(&client, &service, &snapshot_id, output).await,
            SnapshotsCmd::Rollback {
                service,
                snapshot_id,
            } => cmd_snapshots_rollback(&client, &service, &snapshot_id, output).await,
            SnapshotsCmd::Delete {
                service,
                snapshot_id,
            } => cmd_snapshots_delete(&client, &service, &snapshot_id).await,
        },
        Cmd::Alerts(a) => match a {
            AlertsCmd::List { service } => {
                cmd_alerts_list(&client, &service, output).await
            }
            AlertsCmd::Create {
                service,
                name,
                metric,
                operator,
                threshold,
                window,
                cooldown,
                channels,
            } => {
                cmd_alerts_create(
                    &client,
                    &service,
                    name,
                    metric,
                    operator,
                    threshold,
                    window,
                    cooldown,
                    channels,
                    output,
                )
                .await
            }
            AlertsCmd::Patch {
                id,
                enabled,
                name,
                threshold,
                window,
                cooldown,
            } => {
                cmd_alerts_patch(&client, &id, enabled, name, threshold, window, cooldown, output)
                    .await
            }
            AlertsCmd::Delete { id } => cmd_alerts_delete(&client, &id).await,
            AlertsCmd::Events { service } => {
                cmd_alerts_events(&client, &service, output).await
            }
            AlertsCmd::Resolve { id } => cmd_alerts_resolve(&client, &id).await,
            AlertsCmd::Test { id } => cmd_alerts_test(&client, &id).await,
        },
        Cmd::IpRules(r) => match r {
            IpRulesCmd::List { service } => {
                cmd_ip_rules_list(&client, &service, output).await
            }
            IpRulesCmd::Create {
                service,
                cidr,
                action,
                description,
                route_id,
                priority,
            } => {
                cmd_ip_rules_create(
                    &client,
                    &service,
                    cidr,
                    action,
                    description,
                    route_id,
                    priority,
                    output,
                )
                .await
            }
            IpRulesCmd::Patch {
                id,
                enabled,
                cidr,
                action,
                description,
                priority,
            } => {
                cmd_ip_rules_patch(
                    &client,
                    &id,
                    enabled,
                    cidr,
                    action,
                    description,
                    priority,
                    output,
                )
                .await
            }
            IpRulesCmd::Delete { id } => cmd_ip_rules_delete(&client, &id).await,
            IpRulesCmd::Bulk { service, file } => {
                cmd_ip_rules_bulk(&client, &service, &file, output).await
            }
        },
        Cmd::ErrorPages(e) => match e {
            ErrorPagesCmd::List { service } => {
                cmd_error_pages_list(&client, &service, output).await
            }
            ErrorPagesCmd::Create {
                service,
                status_code,
                content_type,
                body,
            } => {
                cmd_error_pages_create(&client, &service, status_code, content_type, body, output)
                    .await
            }
            ErrorPagesCmd::Patch {
                id,
                content_type,
                body,
                enabled,
            } => cmd_error_pages_patch(&client, &id, content_type, body, enabled, output).await,
            ErrorPagesCmd::Delete { id } => cmd_error_pages_delete(&client, &id).await,
        },
        Cmd::ConsumerGroups(cg) => match cg {
            ConsumerGroupsCmd::List { service } => {
                cmd_consumer_groups_list(&client, &service, output).await
            }
            ConsumerGroupsCmd::Create {
                service,
                name,
                description,
                rate_limit,
                rate_window,
                burst_limit,
            } => {
                cmd_consumer_groups_create(
                    &client,
                    &service,
                    name,
                    description,
                    rate_limit,
                    rate_window,
                    burst_limit,
                    output,
                )
                .await
            }
            ConsumerGroupsCmd::Patch {
                id,
                name,
                description,
                rate_limit,
                rate_window,
                burst_limit,
            } => {
                cmd_consumer_groups_patch(
                    &client,
                    &id,
                    name,
                    description,
                    rate_limit,
                    rate_window,
                    burst_limit,
                    output,
                )
                .await
            }
            ConsumerGroupsCmd::Delete { id } => {
                cmd_consumer_groups_delete(&client, &id).await
            }
            ConsumerGroupsCmd::Members { id } => {
                cmd_consumer_groups_members(&client, &id, output).await
            }
            ConsumerGroupsCmd::AddMember { id, consumer_id } => {
                cmd_consumer_groups_add_member(&client, &id, consumer_id, output).await
            }
            ConsumerGroupsCmd::RemoveMember { id, consumer_id } => {
                cmd_consumer_groups_remove_member(&client, &id, &consumer_id).await
            }
            ConsumerGroupsCmd::Usage { id } => {
                cmd_consumer_groups_usage(&client, &id, output).await
            }
        },
        Cmd::Graph { service } => cmd_graph(&client, &service, output).await,
        Cmd::Docs(d) => match d {
            DocsCmd::Show { service } => {
                cmd_docs_show(&client, &service, output).await
            }
            DocsCmd::Try {
                method,
                url,
                headers,
                body,
            } => cmd_docs_try(&client, method, url, headers, body, output).await,
        },
        Cmd::ProxyReplay {
            method,
            url,
            headers,
            body,
        } => cmd_proxy_replay(&client, method, url, headers, body, output).await,
        Cmd::Config(c) => cmd_config(c),
        Cmd::Completion { shell } => {
            let mut cmd = Cli::command();
            let bin = cmd.get_name().to_string();
            generate(shell, &mut cmd, bin, &mut io::stdout());
            Ok(())
        }
        Cmd::Start { args } => cmd_start(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_error(&e);
            ExitCode::from(1)
        }
    }
}

