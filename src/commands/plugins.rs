//! `lynxx plugins`, `lynxx wasm`, `lynxx scripts` — plugin management.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::PluginInstance;
use crate::output::{emit, fmt_id, green, print_table};

use super::{plugin_scope_label, resolve_plugin_scope, service_name_map};

/// Gateway hard-limit for WASM module uploads.
pub const WASM_MAX_BYTES: u64 = 5 * 1024 * 1024;

// ── plugins ──────────────────────────────────────────────────────────────

pub async fn cmd_plugins_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    if fmt != OutputFormat::Table {
        return emit(&plugins, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = plugins
        .iter()
        .map(|p| {
            vec![
                fmt_id(&p.id),
                p.name.clone(),
                plugin_scope_label(p, &by_id),
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "SCOPE", "ENABLED"], &rows);
    Ok(())
}

pub async fn cmd_plugins_install(
    client: &Client,
    name: String,
    service: Option<String>,
    route: Option<String>,
    config: String,
    fmt: OutputFormat,
) -> Result<()> {
    let cfg: Json =
        serde_json::from_str(&config).context("--config must be a valid JSON object")?;
    let (service_id, route_id) = resolve_plugin_scope(client, service, route).await?;
    let body = serde_json::json!({
        "name": name, "service_id": service_id, "route_id": route_id,
        "enabled": true, "config": cfg,
    });
    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} installed plugin {} ({})",
            green("\u{2713}"),
            plugin.name,
            fmt_id(&plugin.id)
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

pub async fn cmd_plugins_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("plugin id must be a UUID")?;
    client.delete(&format!("/plugins/{uuid}")).await?;
    println!("{} deleted plugin {}", green("\u{2713}"), fmt_id(&uuid));
    Ok(())
}

// ── wasm ─────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn cmd_wasm_install(
    client: &Client,
    path: PathBuf,
    name: Option<String>,
    config: Option<String>,
    service: Option<String>,
    route: Option<String>,
    fuel: u64,
    memory_kb: u32,
    wall_ms: u64,
    fmt: OutputFormat,
) -> Result<()> {
    use base64::Engine as _;

    let meta = fs::metadata(&path).with_context(|| format!("read {}", path.display()))?;
    if meta.len() > WASM_MAX_BYTES {
        bail!(
            "wasm module is {} bytes; gateway hard-limit is {} (5MB)",
            meta.len(),
            WASM_MAX_BYTES
        );
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    if !bytes.starts_with(b"\0asm") {
        bail!(
            "{} is not a WebAssembly module (missing \\0asm magic)",
            path.display()
        );
    }

    let display_name = name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm-plugin")
            .to_string()
    });

    let user_config: Option<Json> = match config {
        Some(s) => Some(serde_json::from_str(&s).context("--config must be valid JSON")?),
        None => None,
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut wasm_config = serde_json::json!({
        "module_b64": b64,
        "fuel_per_request": fuel,
        "memory_max_kb": memory_kb,
        "wall_timeout_ms": wall_ms,
    });
    if let Some(uc) = user_config {
        wasm_config["user_config"] = uc;
    }

    let (service_id, route_id) = resolve_plugin_scope(client, service, route).await?;

    let body = serde_json::json!({
        "name": "wasm",
        "service_id": service_id,
        "route_id": route_id,
        "enabled": true,
        "config": wasm_config,
    });
    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} installed wasm plugin {} ({}, {} bytes from {})",
            green("\u{2713}"),
            display_name,
            fmt_id(&plugin.id),
            bytes.len(),
            path.display(),
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

pub async fn cmd_wasm_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let wasm_only: Vec<PluginInstance> = plugins.into_iter().filter(|p| p.name == "wasm").collect();
    if fmt != OutputFormat::Table {
        return emit(&wasm_only, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = wasm_only
        .iter()
        .map(|p| {
            vec![
                fmt_id(&p.id),
                plugin_scope_label(p, &by_id),
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SCOPE", "ENABLED"], &rows);
    Ok(())
}

pub async fn cmd_wasm_remove(client: &Client, id: &str) -> Result<()> {
    cmd_plugins_delete(client, id).await
}

// ── scripts ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn cmd_scripts_install(
    client: &Client,
    on_request_cmd: Option<String>,
    on_request_args: Vec<String>,
    on_response_cmd: Option<String>,
    on_response_args: Vec<String>,
    timeout: u64,
    env: Vec<(String, String)>,
    max_body_bytes: u64,
    service: Option<String>,
    route: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    if on_request_cmd.is_none() && on_response_cmd.is_none() {
        bail!("at least one of --on-request-cmd or --on-response-cmd is required");
    }

    let mut config = serde_json::json!({
        "max_body_bytes": max_body_bytes,
    });

    if let Some(cmd) = &on_request_cmd {
        config["on_request"] = serde_json::json!({
            "command": cmd,
            "args": on_request_args,
            "timeout_ms": timeout,
        });
    }

    if let Some(cmd) = &on_response_cmd {
        config["on_response"] = serde_json::json!({
            "command": cmd,
            "args": on_response_args,
            "timeout_ms": timeout,
        });
    }

    if !env.is_empty() {
        let env_map: serde_json::Map<String, Json> =
            env.into_iter().map(|(k, v)| (k, Json::String(v))).collect();
        config["env"] = Json::Object(env_map);
    }

    let (service_id, route_id) = resolve_plugin_scope(client, service, route).await?;

    let body = serde_json::json!({
        "name": "script-runner",
        "service_id": service_id,
        "route_id": route_id,
        "enabled": true,
        "config": config,
    });

    let plugin: PluginInstance = client.post("/plugins", &body).await?;
    if fmt == OutputFormat::Table {
        let hooks: Vec<&str> = [
            on_request_cmd.as_deref().map(|_| "on_request"),
            on_response_cmd.as_deref().map(|_| "on_response"),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!(
            "{} installed script-runner plugin ({}, hooks: {})",
            green("\u{2713}"),
            fmt_id(&plugin.id),
            hooks.join("+"),
        );
    } else {
        emit(&plugin, fmt)?;
    }
    Ok(())
}

pub async fn cmd_scripts_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let scripts_only: Vec<PluginInstance> = plugins
        .into_iter()
        .filter(|p| p.name == "script-runner")
        .collect();
    if fmt != OutputFormat::Table {
        return emit(&scripts_only, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = scripts_only
        .iter()
        .map(|p| {
            let scope = plugin_scope_label(p, &by_id);
            let hooks = {
                let mut h = Vec::new();
                if p.config.get("on_request").is_some() {
                    h.push("req");
                }
                if p.config.get("on_response").is_some() {
                    h.push("res");
                }
                if h.is_empty() {
                    "none".to_string()
                } else {
                    h.join("+")
                }
            };
            vec![
                fmt_id(&p.id),
                scope,
                hooks,
                if p.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SCOPE", "HOOKS", "ENABLED"], &rows);
    Ok(())
}

pub async fn cmd_scripts_delete(client: &Client, id: &str) -> Result<()> {
    cmd_plugins_delete(client, id).await
}
