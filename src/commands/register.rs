//! `lynxx register` — read `lynxx.toml` and register services + routes.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::{Route, Service};
use crate::output::{fmt_id, green};
use crate::resolve::resolve_service;

// ---------------------------------------------------------------------------
// lynxx.toml schema (only the fields register cares about)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProjectFile {
    #[serde(default)]
    services: Vec<FileSvc>,
    #[serde(default)]
    routes: Vec<FileRoute>,
}

#[derive(Debug, Deserialize)]
struct FileSvc {
    name: String,
    url: String,
    #[serde(default = "default_retries")]
    retries: u32,
    #[serde(default = "default_timeout")]
    connect_timeout_ms: u64,
    #[serde(default)]
    lb_strategy: Option<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    subdomain: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileRoute {
    service: String,
    paths: Vec<String>,
    #[serde(default = "default_methods")]
    methods: Vec<String>,
    #[serde(default)]
    strip_path: bool,
}

fn default_retries() -> u32 { 3 }
fn default_timeout() -> u64 { 5000 }
fn default_methods() -> Vec<String> { vec!["GET".into()] }

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub async fn cmd_register(
    client: &Client,
    file: PathBuf,
    fmt: OutputFormat,
) -> Result<()> {
    let raw = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let project: ProjectFile = toml::from_str(&raw)
        .with_context(|| format!("parsing {}", file.display()))?;

    if project.services.is_empty() && project.routes.is_empty() {
        bail!("no [[services]] or [[routes]] found in {}", file.display());
    }

    let mut svc_count = 0u32;
    let mut route_count = 0u32;

    for svc in &project.services {
        let existing = resolve_service(client, &svc.name).await.ok();
        let is_update = existing.is_some();

        let registered: Service = if let Some(existing) = existing {
            let mut body = serde_json::json!({
                "name": svc.name,
                "url": svc.url,
                "retries": svc.retries,
                "connect_timeout_ms": svc.connect_timeout_ms,
            });
            set_opt(&mut body, "lb_strategy", &svc.lb_strategy);
            set_opt(&mut body, "domain", &svc.domain);
            set_opt(&mut body, "subdomain", &svc.subdomain);
            client
                .put(&format!("/services/{}", existing.id), &body)
                .await
                .with_context(|| format!("updating service {}", svc.name))?
        } else {
            let mut body = serde_json::json!({
                "name": svc.name,
                "url": svc.url,
                "retries": svc.retries,
                "connect_timeout_ms": svc.connect_timeout_ms,
            });
            set_opt(&mut body, "lb_strategy", &svc.lb_strategy);
            set_opt(&mut body, "domain", &svc.domain);
            set_opt(&mut body, "subdomain", &svc.subdomain);
            client
                .post("/services", &body)
                .await
                .with_context(|| format!("creating service {}", svc.name))?
        };

        if fmt == OutputFormat::Table {
            let verb = if is_update { "updated" } else { "created" };
            println!(
                "{} {} service {} ({})",
                green("✓"), verb, registered.name, fmt_id(&registered.id),
            );
        }
        svc_count += 1;
    }

    for rt in &project.routes {
        let svc = resolve_service(client, &rt.service)
            .await
            .with_context(|| format!("route references unknown service '{}'", rt.service))?;

        let existing_routes: Vec<Route> = client.get("/routes").await.unwrap_or_default();
        let dup = existing_routes.iter().find(|r| {
            r.service_id == svc.id && r.paths == rt.paths
        });

        let registered: Route = if let Some(existing) = dup {
            let body = serde_json::json!({
                "service_id": svc.id,
                "paths": rt.paths,
                "methods": rt.methods,
                "strip_path": rt.strip_path,
            });
            client
                .put(&format!("/routes/{}", existing.id), &body)
                .await
                .with_context(|| format!("updating route {:?}", rt.paths))?
        } else {
            let body = serde_json::json!({
                "service_id": svc.id,
                "paths": rt.paths,
                "methods": rt.methods,
                "strip_path": rt.strip_path,
            });
            client
                .post("/routes", &body)
                .await
                .with_context(|| format!("creating route {:?}", rt.paths))?
        };

        if fmt == OutputFormat::Table {
            let verb = if dup.is_some() { "updated" } else { "created" };
            println!(
                "{} {} route {} → {} ({})",
                green("✓"),
                verb,
                rt.paths.join(", "),
                rt.service,
                fmt_id(&registered.id),
            );
        }
        route_count += 1;
    }

    if fmt == OutputFormat::Table {
        println!(
            "\n{} registered {} service(s), {} route(s)",
            green("✓"), svc_count, route_count,
        );
    }

    Ok(())
}

fn set_opt(body: &mut Json, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        body[key] = Json::String(v.clone());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_toml() {
        let raw = r#"
[[services]]
name = "api"
url = "http://localhost:3000"
"#;
        let pf: ProjectFile = toml::from_str(raw).unwrap();
        assert_eq!(pf.services.len(), 1);
        assert_eq!(pf.services[0].name, "api");
        assert_eq!(pf.services[0].retries, 3);
        assert_eq!(pf.services[0].connect_timeout_ms, 5000);
        assert!(pf.routes.is_empty());
    }

    #[test]
    fn parse_full_toml() {
        let raw = r#"
[gateway]
proxy_port = 8000
admin_port = 8001

[[services]]
name = "users"
url = "http://127.0.0.1:4000"
retries = 5
connect_timeout_ms = 10000
lb_strategy = "least-connections"

[[services]]
name = "billing"
url = "http://127.0.0.1:5000"

[[routes]]
service = "users"
paths = ["/users", "/auth"]
methods = ["GET", "POST", "PUT"]
strip_path = true

[[routes]]
service = "billing"
paths = ["/billing"]
"#;
        let pf: ProjectFile = toml::from_str(raw).unwrap();
        assert_eq!(pf.services.len(), 2);
        assert_eq!(pf.services[0].lb_strategy.as_deref(), Some("least-connections"));
        assert_eq!(pf.services[1].retries, 3);
        assert_eq!(pf.routes.len(), 2);
        assert_eq!(pf.routes[0].paths, vec!["/users", "/auth"]);
        assert_eq!(pf.routes[0].methods, vec!["GET", "POST", "PUT"]);
        assert!(pf.routes[0].strip_path);
        assert_eq!(pf.routes[1].methods, vec!["GET"]);
        assert!(!pf.routes[1].strip_path);
    }

    #[test]
    fn parse_empty_is_error() {
        let raw = r#"
[gateway]
proxy_port = 8000
"#;
        let pf: ProjectFile = toml::from_str(raw).unwrap();
        assert!(pf.services.is_empty());
        assert!(pf.routes.is_empty());
    }

    #[test]
    fn parse_rejects_bad_toml() {
        let raw = "[[services\nname = broken";
        let result: Result<ProjectFile, _> = toml::from_str(raw);
        assert!(result.is_err());
    }

    #[test]
    fn defaults_applied() {
        let raw = r#"
[[services]]
name = "x"
url = "http://localhost:1234"

[[routes]]
service = "x"
paths = ["/foo"]
"#;
        let pf: ProjectFile = toml::from_str(raw).unwrap();
        assert_eq!(pf.services[0].retries, 3);
        assert_eq!(pf.services[0].connect_timeout_ms, 5000);
        assert_eq!(pf.routes[0].methods, vec!["GET"]);
        assert!(!pf.routes[0].strip_path);
    }
}
