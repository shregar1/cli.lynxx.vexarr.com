//! `lynxx routes` — CRUD for gateway routes plus route-test.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::{Route, RouteTestResponse};
use crate::output::{bold, emit, fmt_id, green, print_table, red, yellow};
use crate::resolve::resolve_service;

use super::service_name_map;

pub async fn cmd_routes_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let routes: Vec<Route> = client.get("/routes").await?;
    if fmt != OutputFormat::Table {
        return emit(&routes, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = routes
        .iter()
        .map(|r| {
            vec![
                fmt_id(&r.id),
                by_id
                    .get(&r.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&r.service_id)),
                r.paths.join(","),
                if r.methods.is_empty() {
                    "*".into()
                } else {
                    r.methods.join(",")
                },
                if r.strip_path {
                    "yes".into()
                } else {
                    "no".into()
                },
            ]
        })
        .collect();
    print_table(&["ID", "SERVICE", "PATHS", "METHODS", "STRIP"], &rows);
    Ok(())
}

pub async fn cmd_routes_get(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    let route: Route = client.get(&format!("/routes/{uuid}")).await?;
    emit(&route, fmt)
}

pub async fn cmd_routes_create(
    client: &Client,
    service: String,
    paths: Vec<String>,
    methods: Vec<String>,
    strip_path: bool,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, &service).await?;
    let body = serde_json::json!({
        "paths": paths, "methods": methods,
        "service_id": svc.id, "strip_path": strip_path,
    });
    let route: Route = client.post("/routes", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created route {} \u{2192} {} (paths: {})",
            green("\u{2713}"),
            fmt_id(&route.id),
            svc.name,
            route.paths.join(",")
        );
    } else {
        emit(&route, fmt)?;
    }
    Ok(())
}

pub async fn cmd_routes_update(
    client: &Client,
    id: &str,
    service: String,
    paths: Vec<String>,
    methods: Vec<String>,
    strip_path: bool,
    fmt: OutputFormat,
) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    let svc = resolve_service(client, &service).await?;
    let body = serde_json::json!({
        "service_id": svc.id,
        "paths": paths,
        "methods": methods,
        "strip_path": strip_path,
    });
    let updated: Route = client.put(&format!("/routes/{uuid}"), &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} updated route {} (paths: {})",
            green("\u{2713}"),
            fmt_id(&updated.id),
            updated.paths.join(", ")
        );
    } else {
        emit(&updated, fmt)?;
    }
    Ok(())
}

pub async fn cmd_routes_patch(
    client: &Client,
    id: &str,
    service: Option<String>,
    paths: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    strip_path: Option<bool>,
    fmt: OutputFormat,
) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    let mut body = serde_json::Map::new();
    if let Some(svc_name) = service {
        let svc = resolve_service(client, &svc_name).await?;
        body.insert("service_id".into(), Json::String(svc.id.to_string()));
    }
    if let Some(p) = paths {
        body.insert("paths".into(), serde_json::to_value(p)?);
    }
    if let Some(m) = methods {
        body.insert("methods".into(), serde_json::to_value(m)?);
    }
    if let Some(sp) = strip_path {
        body.insert("strip_path".into(), Json::Bool(sp));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to patch");
    }
    let updated: Route = client
        .patch_json(&format!("/routes/{uuid}"), &Json::Object(body))
        .await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} patched route {} (paths: {})",
            green("\u{2713}"),
            fmt_id(&updated.id),
            updated.paths.join(", ")
        );
    } else {
        emit(&updated, fmt)?;
    }
    Ok(())
}

pub async fn cmd_routes_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    client.delete(&format!("/routes/{uuid}")).await?;
    println!("{} deleted route {}", green("\u{2713}"), fmt_id(&uuid));
    Ok(())
}

pub async fn cmd_routes_test(
    client: &Client,
    id: &str,
    method: &str,
    path: &str,
    raw_headers: &[String],
    fmt: OutputFormat,
) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("route id must be a UUID")?;
    let mut headers = HashMap::new();
    for h in raw_headers {
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let body = serde_json::json!({
        "method": method,
        "path": path,
        "headers": headers,
    });
    let res: RouteTestResponse = client
        .post(&format!("/routes/{uuid}/test"), &body)
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&res, fmt);
    }
    fn status_color(s: u16) -> String {
        let txt = s.to_string();
        if s == 0 {
            red(&txt)
        } else if s < 400 {
            green(&txt)
        } else if s < 500 {
            yellow(&txt)
        } else {
            red(&txt)
        }
    }
    println!("{}", bold("External (through gateway)"));
    println!("  URL:     {}", res.external.url);
    println!(
        "  Status:  {} ({}ms)",
        status_color(res.external.status),
        res.external.latency_ms
    );
    println!(
        "  Healthy: {}",
        if res.external.healthy {
            green("yes")
        } else {
            red("no")
        }
    );
    println!();
    println!("{}", bold("Internal (direct upstream)"));
    println!("  URL:     {}", res.internal.url);
    println!(
        "  Status:  {} ({}ms)",
        status_color(res.internal.status),
        res.internal.latency_ms
    );
    println!(
        "  Healthy: {}",
        if res.internal.healthy {
            green("yes")
        } else {
            red("no")
        }
    );
    if res.external.status != res.internal.status {
        println!();
        println!(
            "{} status mismatch: external={} internal={} (gateway is modifying the response)",
            yellow("\u{26a0}"),
            res.external.status,
            res.internal.status
        );
    }
    Ok(())
}
