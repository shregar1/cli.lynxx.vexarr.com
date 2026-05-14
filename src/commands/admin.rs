//! Administrative commands: audit, circuit breakers, health, reload, quota,
//! upstreams, metrics, ACME, debug, live-config, deploy, and logs.

use anyhow::{Context, Result};
use chrono::Local;
use serde_json::Value as Json;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::*;
use crate::output::{bold, dim, emit, fmt_id, green, print_table, red};
use crate::resolve::resolve_service;

use super::service_name_map;

// ── Audit ────────────────────────────────────────────────────────────────

pub async fn cmd_audit_list(
    client: &Client,
    action: Option<String>,
    resource_type: Option<String>,
    limit: u32,
    fmt: OutputFormat,
) -> Result<()> {
    let mut qs = vec![format!("limit={limit}")];
    if let Some(a) = &action {
        qs.push(format!("action={a}"));
    }
    if let Some(rt) = &resource_type {
        qs.push(format!("resource_type={rt}"));
    }
    let path = format!("/audit?{}", qs.join("&"));
    let entries: Vec<AuditEntry> = client.get(&path).await?;
    if fmt != OutputFormat::Table {
        return emit(&entries, fmt);
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                e.ts.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
                e.actor_name.clone().unwrap_or_default(),
                e.action.clone(),
                e.resource_type.clone(),
                e.resource_id.clone().unwrap_or_default(),
            ]
        })
        .collect();
    print_table(
        &["ID", "TIMESTAMP", "ACTOR", "ACTION", "TYPE", "RESOURCE_ID"],
        &rows,
    );
    Ok(())
}

pub async fn cmd_audit_verify(client: &Client, fmt: OutputFormat) -> Result<()> {
    let result: AuditVerifyResult = client.get("/audit/verify").await?;
    if fmt != OutputFormat::Table {
        return emit(&result, fmt);
    }
    if result.ok {
        println!(
            "{} audit chain intact ({} entries verified)",
            green("\u{2713}"),
            result.checked
        );
    } else {
        println!(
            "{} audit chain broken at entry {}",
            red("\u{2717}"),
            result.broken_at.unwrap_or(0)
        );
    }
    Ok(())
}

// ── Circuit breakers ─────────────────────────────────────────────────────

pub async fn cmd_circuit_breakers_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let cbs: Vec<ServiceHealth> = client.get("/circuit-breakers").await?;
    if fmt != OutputFormat::Table {
        return emit(&cbs, fmt);
    }
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let svc_name = |id: &Uuid| {
        services
            .iter()
            .find(|s| s.id == *id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| fmt_id(id))
    };
    let rows: Vec<Vec<String>> = cbs
        .iter()
        .map(|cb| {
            vec![
                svc_name(&cb.service_id),
                cb.status.clone(),
                cb.circuit.clone(),
                cb.consecutive_failures.to_string(),
                cb.last_error.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    print_table(
        &["SERVICE", "STATUS", "CIRCUIT", "FAILS", "LAST_ERROR"],
        &rows,
    );
    Ok(())
}

pub async fn cmd_circuit_breakers_reset(client: &Client, service: &str) -> Result<()> {
    let svc = resolve_service(client, service).await?;
    client
        .post_empty(&format!("/circuit-breakers/{}/reset", svc.id))
        .await?;
    println!(
        "{} reset circuit breaker for {} ({})",
        green("\u{2713}"),
        svc.name,
        fmt_id(&svc.id)
    );
    Ok(())
}

// ── Health / Reload ──────────────────────────────────────────────────────

pub async fn cmd_health(client: &Client, fmt: OutputFormat) -> Result<()> {
    let health: Vec<ServiceHealth> = client.get("/health").await?;
    if fmt != OutputFormat::Table {
        return emit(&health, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = health
        .iter()
        .map(|h| {
            vec![
                by_id
                    .get(&h.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&h.service_id)),
                h.status.clone(),
                h.circuit.clone(),
                h.consecutive_failures.to_string(),
                h.last_check_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "\u{2014}".into()),
                h.last_error
                    .clone()
                    .unwrap_or_else(|| "\u{2014}".into())
                    .chars()
                    .take(60)
                    .collect(),
            ]
        })
        .collect();
    print_table(
        &[
            "SERVICE",
            "STATUS",
            "CIRCUIT",
            "FAILS",
            "LAST_CHECK",
            "LAST_ERROR",
        ],
        &rows,
    );
    Ok(())
}

pub async fn cmd_reload(client: &Client) -> Result<()> {
    let v = client.post_empty("/reload").await?;
    println!("{} reload triggered", green("\u{2713}"));
    if !v.is_null() {
        println!("{}", dim(&serde_json::to_string_pretty(&v)?));
    }
    Ok(())
}

// ── Quota / Upstreams / Metrics ──────────────────────────────────────────

pub async fn cmd_quota(client: &Client, fmt: OutputFormat) -> Result<()> {
    let q: Json = client.get("/quota").await?;
    if fmt != OutputFormat::Table {
        return emit(&q, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&q)?);
    Ok(())
}

pub async fn cmd_upstreams(client: &Client, fmt: OutputFormat) -> Result<()> {
    let u: Json = client.get("/upstreams").await?;
    if fmt != OutputFormat::Table {
        return emit(&u, fmt);
    }
    let total = u.get("total_targets").and_then(|v| v.as_u64()).unwrap_or(0);
    let healthy = u
        .get("healthy_targets")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let unhealthy = u
        .get("unhealthy_targets")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!(
        "total: {}  healthy: {}  unhealthy: {}",
        bold(&total.to_string()),
        green(&healthy.to_string()),
        if unhealthy > 0 {
            red(&unhealthy.to_string())
        } else {
            unhealthy.to_string()
        }
    );
    if let Some(targets) = u.get("targets").and_then(|v| v.as_array()) {
        let rows: Vec<Vec<String>> = targets
            .iter()
            .map(|t| {
                vec![
                    t.get("target_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s[..8].to_string())
                        .unwrap_or_default(),
                    t.get("service_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    t.get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    t.get("weight")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1)
                        .to_string(),
                    if t.get("healthy").and_then(|v| v.as_bool()).unwrap_or(false) {
                        green("yes")
                    } else {
                        red("no")
                    },
                ]
            })
            .collect();
        if !rows.is_empty() {
            print_table(&["ID", "SERVICE", "URL", "WEIGHT", "HEALTHY"], &rows);
        }
    }
    Ok(())
}

pub async fn cmd_metrics(client: &Client) -> Result<()> {
    let text = client.get_text("/metrics").await?;
    print!("{text}");
    Ok(())
}

// ── ACME ─────────────────────────────────────────────────────────────────

pub async fn cmd_acme_status(client: &Client, fmt: OutputFormat) -> Result<()> {
    let st: AcmeStatusView = client.get("/acme/status").await?;
    if fmt != OutputFormat::Table {
        return emit(&st, fmt);
    }
    println!(
        "enabled: {}  has_cert: {}  http01: {}  tls-alpn01: {}",
        if st.enabled {
            green("yes")
        } else {
            dim("no")
        },
        if st.has_cert.unwrap_or(false) {
            green("yes")
        } else {
            dim("no")
        },
        st.http01_challenges.unwrap_or(0),
        st.tls_alpn01_challenges.unwrap_or(0),
    );
    Ok(())
}

pub async fn cmd_acme_renew(client: &Client) -> Result<()> {
    let v = client.post_empty("/acme/renew").await?;
    println!("{} renewal triggered", green("\u{2713}"));
    if !v.is_null() {
        println!("{}", dim(&serde_json::to_string_pretty(&v)?));
    }
    Ok(())
}

pub async fn cmd_acme_revoke(client: &Client) -> Result<()> {
    let v = client.post_empty("/acme/revoke").await?;
    println!("{} revocation triggered", green("\u{2713}"));
    if !v.is_null() {
        println!("{}", dim(&serde_json::to_string_pretty(&v)?));
    }
    Ok(())
}

pub async fn cmd_acme_set_http01(
    client: &Client,
    token: String,
    key_authorization: String,
) -> Result<()> {
    let body = serde_json::json!({ "token": token, "key_authorization": key_authorization });
    let _: Json = client.post("/acme/http-01-challenges", &body).await?;
    println!(
        "{} set HTTP-01 challenge for token {}",
        green("\u{2713}"),
        token
    );
    Ok(())
}

pub async fn cmd_acme_clear_http01(client: &Client, token: &str) -> Result<()> {
    client
        .delete(&format!("/acme/http-01-challenges/{token}"))
        .await?;
    println!(
        "{} cleared HTTP-01 challenge for {}",
        green("\u{2713}"),
        token
    );
    Ok(())
}

pub async fn cmd_acme_set_tls_alpn01(
    client: &Client,
    domain: String,
    key_authorization: String,
) -> Result<()> {
    let body = serde_json::json!({ "domain": domain, "key_authorization": key_authorization });
    let _: Json = client
        .post("/acme/tls-alpn-01-challenges", &body)
        .await?;
    println!(
        "{} set TLS-ALPN-01 challenge for {}",
        green("\u{2713}"),
        domain
    );
    Ok(())
}

pub async fn cmd_acme_clear_tls_alpn01(client: &Client, domain: &str) -> Result<()> {
    client
        .delete(&format!("/acme/tls-alpn-01-challenges/{domain}"))
        .await?;
    println!(
        "{} cleared TLS-ALPN-01 challenge for {}",
        green("\u{2713}"),
        domain
    );
    Ok(())
}

// ── Debug / profiling ────────────────────────────────────────────────────

pub async fn cmd_debug(client: &Client, endpoint: &str, fmt: OutputFormat) -> Result<()> {
    let data: Json = client.get(&format!("/debug/pprof/{endpoint}")).await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

// ── Live config ──────────────────────────────────────────────────────────

pub async fn cmd_live_config_get(client: &Client, fmt: OutputFormat) -> Result<()> {
    let data: Json = client.get("/admin/v1/config").await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

pub async fn cmd_live_config_set(
    client: &Client,
    path: &str,
    value_str: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let value: Json = serde_json::from_str(value_str)
        .context("value must be valid JSON (e.g. '\"debug\"', '9000', 'true')")?;
    let api_path = format!("/admin/v1/config/{}", path.replace('.', "/"));
    let body = serde_json::json!({ "value": value });
    let result: Json = client.patch_json(&api_path, &body).await?;
    if fmt != OutputFormat::Table {
        return emit(&result, fmt);
    }
    println!(
        "{} config set {} = {}",
        green("\u{2713}"),
        path,
        value_str
    );
    Ok(())
}

pub async fn cmd_live_config_lookup(client: &Client, tag: &str, fmt: OutputFormat) -> Result<()> {
    let data: Json = client
        .get(&format!("/admin/v1/config/@id/{tag}"))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

pub async fn cmd_live_config_validate(
    client: &Client,
    body_str: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let body: Json =
        serde_json::from_str(body_str).context("body must be valid JSON")?;
    let result: Json = client.post("/admin/v1/config/validate", &body).await?;
    if fmt != OutputFormat::Table {
        return emit(&result, fmt);
    }
    let valid = result
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if valid {
        println!("{} configuration is valid", green("\u{2713}"));
    } else {
        let err = result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        println!("{} configuration invalid: {}", red("\u{2717}"), err);
    }
    Ok(())
}

pub async fn cmd_live_config_history(client: &Client, fmt: OutputFormat) -> Result<()> {
    let data: Json = client.get("/admin/v1/config/history").await?;
    if fmt != OutputFormat::Table {
        return emit(&data, fmt);
    }
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

// ── Deploy ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn cmd_deploy_start(
    client: &Client,
    service_ident: &str,
    target_url: String,
    health_path: String,
    health_interval_ms: u64,
    health_timeout_ms: u64,
    health_threshold: u32,
    drain_timeout_ms: u64,
    weight: u32,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service_ident).await?;
    let body = serde_json::json!({
        "new_target_url": target_url,
        "health_check_path": health_path,
        "health_check_interval_ms": health_interval_ms,
        "health_check_timeout_ms": health_timeout_ms,
        "health_check_threshold": health_threshold,
        "drain_timeout_ms": drain_timeout_ms,
        "weight": weight,
    });
    let res: DeployResponse = client
        .post(&format!("/services/{}/deploy", svc.id), &body)
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&res, fmt);
    }
    println!(
        "{} deploy started for service {} ({})",
        green("\u{2713}"),
        svc.name,
        fmt_id(&svc.id)
    );
    println!("  Deploy ID:    {}", fmt_id(&res.deploy_id));
    println!("  New target:   {}", fmt_id(&res.new_target_id));
    println!("  State:        {}", res.state);
    println!("  Health gates: 0/{}", res.health_checks_required);
    Ok(())
}

pub async fn cmd_deploy_status(
    client: &Client,
    service_ident: &str,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, service_ident).await?;
    let res: DeployStatusResponse = client
        .get(&format!("/services/{}/deploy/status", svc.id))
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&res, fmt);
    }
    println!("{}", bold(&format!("Deploy for service {}", svc.name)));
    println!("  State:        {}", res.state);
    println!("  New target:   {}", fmt_id(&res.new_target_id));
    println!(
        "  Health:       {}/{}",
        res.health_checks_passed, res.health_checks_required
    );
    println!("  Started:      {}", res.started_at);
    if !res.old_target_ids.is_empty() {
        let ids: Vec<String> = res.old_target_ids.iter().map(fmt_id).collect();
        println!("  Old targets:  {}", ids.join(", "));
    }
    Ok(())
}

// ── Logs ─────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn cmd_logs_list(
    client: &Client,
    service_id: Option<Uuid>,
    route_id: Option<Uuid>,
    project_id: Option<Uuid>,
    level: Option<String>,
    search: Option<String>,
    limit: u32,
    fmt: OutputFormat,
) -> Result<()> {
    let mut params = vec![format!("limit={limit}")];
    if let Some(v) = service_id {
        params.push(format!("service_id={v}"));
    }
    if let Some(v) = route_id {
        params.push(format!("route_id={v}"));
    }
    if let Some(v) = project_id {
        params.push(format!("project_id={v}"));
    }
    if let Some(v) = &level {
        params.push(format!("level={v}"));
    }
    if let Some(v) = &search {
        params.push(format!("search={v}"));
    }
    let qs = params.join("&");
    let logs: Vec<LogEntry> = client.get(&format!("/logs?{qs}")).await?;
    if fmt != OutputFormat::Table {
        return emit(&logs, fmt);
    }
    let rows: Vec<Vec<String>> = logs
        .iter()
        .map(|l| {
            let status_str = l
                .status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into());
            let dur = l
                .duration_ms
                .map(|d| format!("{d}ms"))
                .unwrap_or_else(|| "-".into());
            vec![
                l.ts.with_timezone(&Local)
                    .format("%H:%M:%S")
                    .to_string(),
                l.level.clone(),
                l.method.clone().unwrap_or_default(),
                l.path.clone().unwrap_or_default(),
                status_str,
                dur,
                l.client_ip.clone().unwrap_or_default(),
                l.message.chars().take(60).collect(),
            ]
        })
        .collect();
    print_table(
        &[
            "TIME", "LEVEL", "METHOD", "PATH", "STATUS", "DURATION", "CLIENT", "MESSAGE",
        ],
        &rows,
    );
    Ok(())
}
