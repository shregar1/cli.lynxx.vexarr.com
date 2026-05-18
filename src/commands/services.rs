//! `lynxx services` — CRUD for gateway services.

use anyhow::{bail, Result};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::Service;
use crate::output::{emit, fmt_id, green, print_table};
use crate::resolve::resolve_service;

use super::emit_success;

pub async fn cmd_services_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let services: Vec<Service> = client.get("/services").await?;
    if fmt != OutputFormat::Table {
        return emit(&services, fmt);
    }
    let rows: Vec<Vec<String>> = services
        .iter()
        .map(|s| {
            vec![
                fmt_id(&s.id),
                s.name.clone(),
                s.url.clone(),
                s.retries.to_string(),
                format!("{}ms", s.connect_timeout_ms),
                s.lb_strategy
                    .clone()
                    .unwrap_or_else(|| "round-robin".into()),
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "URL", "RETRIES", "TIMEOUT", "LB"], &rows);
    Ok(())
}

pub async fn cmd_services_get(client: &Client, ident: &str, fmt: OutputFormat) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    emit(&svc, fmt)
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_services_create(
    client: &Client,
    name: String,
    url: String,
    retries: u32,
    connect_timeout_ms: u64,
    lb_strategy: Option<String>,
    domain: Option<String>,
    subdomain: Option<String>,
    project_id: Option<Uuid>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::json!({
        "name": name, "url": url,
        "retries": retries, "connect_timeout_ms": connect_timeout_ms,
    });
    if let Some(lb) = lb_strategy {
        body["lb_strategy"] = Json::String(lb);
    }
    if let Some(d) = domain {
        body["domain"] = Json::String(d);
    }
    if let Some(s) = subdomain {
        body["subdomain"] = Json::String(s);
    }
    if let Some(p) = project_id {
        body["project_id"] = Json::String(p.to_string());
    }
    let svc: Service = client.post("/services", &body).await?;
    emit_success(&svc, fmt, "created", "service", &svc.name, &svc.id)
}

pub async fn cmd_services_delete(client: &Client, ident: &str) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    client.delete(&format!("/services/{}", svc.id)).await?;
    println!(
        "{} deleted service {} ({})",
        green("✓"),
        svc.name,
        fmt_id(&svc.id)
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_services_update(
    client: &Client,
    ident: &str,
    name: String,
    url: String,
    retries: u32,
    connect_timeout_ms: u64,
    lb_strategy: Option<String>,
    domain: Option<String>,
    subdomain: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    let mut body = serde_json::json!({
        "name": name, "url": url,
        "retries": retries, "connect_timeout_ms": connect_timeout_ms,
    });
    if let Some(lb) = lb_strategy {
        body["lb_strategy"] = Json::String(lb);
    }
    if let Some(d) = domain {
        body["domain"] = Json::String(d);
    }
    if let Some(s) = subdomain {
        body["subdomain"] = Json::String(s);
    }
    let updated: Service = client.put(&format!("/services/{}", svc.id), &body).await?;
    emit_success(
        &updated,
        fmt,
        "updated",
        "service",
        &updated.name,
        &updated.id,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_services_patch(
    client: &Client,
    ident: &str,
    name: Option<String>,
    url: Option<String>,
    retries: Option<u32>,
    connect_timeout_ms: Option<u64>,
    lb_strategy: Option<String>,
    domain: Option<String>,
    subdomain: Option<String>,
    project_id: Option<Uuid>,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, ident).await?;
    let mut body = serde_json::Map::new();
    if let Some(v) = name {
        body.insert("name".into(), Json::String(v));
    }
    if let Some(v) = url {
        body.insert("url".into(), Json::String(v));
    }
    if let Some(v) = retries {
        body.insert("retries".into(), Json::from(v));
    }
    if let Some(v) = connect_timeout_ms {
        body.insert("connect_timeout_ms".into(), Json::from(v));
    }
    if let Some(v) = lb_strategy {
        body.insert("lb_strategy".into(), Json::String(v));
    }
    if let Some(v) = domain {
        body.insert("domain".into(), Json::String(v));
    }
    if let Some(v) = subdomain {
        body.insert("subdomain".into(), Json::String(v));
    }
    if let Some(v) = project_id {
        body.insert("project_id".into(), Json::String(v.to_string()));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to patch");
    }
    let updated: Service = client
        .patch_json(&format!("/services/{}", svc.id), &Json::Object(body))
        .await?;
    emit_success(
        &updated,
        fmt,
        "patched",
        "service",
        &updated.name,
        &updated.id,
    )
}
