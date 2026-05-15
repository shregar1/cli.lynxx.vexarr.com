//! Command handler modules — extracted from the monolithic `main.rs`.
//!
//! Each submodule groups related CLI command functions. Shared helpers
//! that were duplicated across commands live here in the root module.

pub mod admin;
pub mod declarative;
pub mod exec;
pub mod live;
pub mod login;
pub mod logout;
pub mod plugins;
pub mod record;
pub mod resources;
pub mod routes;
pub mod services;
pub mod whoami;

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::client::Client;
use crate::cli::OutputFormat;
use crate::models::{PluginInstance, Service};
use crate::output::{emit, fmt_id, green};
use crate::resolve::resolve_service;

// ---------------------------------------------------------------------------
// Shared DRY helpers
// ---------------------------------------------------------------------------

/// Fetch every service and return a `UUID → name` lookup map.
///
/// Used by routes-list, plugins-list, wasm-list, scripts-list, health,
/// targets-list, circuit-breakers-list, and more.
pub(crate) async fn service_name_map(client: &Client) -> HashMap<Uuid, String> {
    let services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    services.iter().map(|s| (s.id, s.name.clone())).collect()
}

/// Human-readable scope label for a plugin instance (e.g. `"service:my-api"`,
/// `"route:ab12cd34"`, `"global"`).
///
/// Used by plugins-list, wasm-list, and scripts-list.
pub(crate) fn plugin_scope_label(
    p: &PluginInstance,
    svc_names: &HashMap<Uuid, String>,
) -> String {
    match (p.service_id, p.route_id) {
        (Some(s), _) => format!(
            "service:{}",
            svc_names
                .get(&s)
                .cloned()
                .unwrap_or_else(|| fmt_id(&s)),
        ),
        (_, Some(r)) => format!("route:{}", fmt_id(&r)),
        _ => "global".into(),
    }
}

/// Resolve optional `--service` / `--route` flags into a
/// `(Option<service_id>, Option<route_id>)` pair.
///
/// Used by plugins-install, wasm-install, and scripts-install.
pub(crate) async fn resolve_plugin_scope(
    client: &Client,
    service: Option<String>,
    route: Option<String>,
) -> Result<(Option<Uuid>, Option<Uuid>)> {
    let service_id = if let Some(s) = service {
        Some(resolve_service(client, &s).await?.id)
    } else {
        None
    };
    let route_id = match route {
        Some(r) => Some(Uuid::parse_str(&r).context("--route must be a UUID")?),
        None => None,
    };
    Ok((service_id, route_id))
}

/// Emit either a structured value (JSON / YAML) or a one-line success
/// message when the output format is `Table`.
///
/// Covers the extremely common pattern:
/// ```text
/// if fmt == Table { println!("{} {action} {kind} {name} ({id})", green("✓")); }
/// else { emit(&val, fmt)?; }
/// ```
pub(crate) fn emit_success<T: Serialize>(
    val: &T,
    fmt: OutputFormat,
    action: &str,
    kind: &str,
    name: &str,
    id: &Uuid,
) -> Result<()> {
    if fmt == OutputFormat::Table {
        println!(
            "{} {} {} {} ({})",
            green("✓"),
            action,
            kind,
            name,
            fmt_id(id),
        );
    } else {
        emit(val, fmt)?;
    }
    Ok(())
}
