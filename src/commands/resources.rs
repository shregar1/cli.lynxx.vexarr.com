//! `lynxx consumers`, `lynxx targets`, `lynxx tokens`, `lynxx projects` — resource CRUD.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use serde_json::Value as Json;
use uuid::Uuid;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::{AdminTokenView, Consumer, Project, Service, Target};
use crate::output::{bold, dim, emit, fmt_id, green, print_table};
use crate::resolve::{resolve_by_name_or_id, resolve_consumer, resolve_project, resolve_service};

use super::{emit_success, service_name_map};

// ── Consumers ────────────────────────────────────────────────────────────

pub async fn cmd_consumers_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    if fmt != OutputFormat::Table {
        return emit(&consumers, fmt);
    }
    let rows: Vec<Vec<String>> = consumers
        .iter()
        .map(|c| {
            vec![
                fmt_id(&c.id),
                c.username.clone(),
                c.custom_id.clone().unwrap_or_default(),
                c.api_key
                    .clone()
                    .map(|k| {
                        let n = k.len().min(8);
                        format!("{}…", &k[..n])
                    })
                    .unwrap_or_default(),
            ]
        })
        .collect();
    print_table(&["ID", "USERNAME", "CUSTOM_ID", "API_KEY"], &rows);
    Ok(())
}

pub async fn cmd_consumers_create(
    client: &Client,
    username: String,
    custom_id: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "username": username, "custom_id": custom_id });
    let consumer: Consumer = client.post("/consumers", &body).await?;
    emit_success(
        &consumer,
        fmt,
        "created",
        "consumer",
        &consumer.username,
        &consumer.id,
    )
}

pub async fn cmd_consumers_delete(client: &Client, ident: &str) -> Result<()> {
    let consumer = resolve_consumer(client, ident).await?;
    client
        .delete(&format!("/consumers/{}", consumer.id))
        .await?;
    println!(
        "{} deleted consumer {} ({})",
        green("\u{2713}"),
        consumer.username,
        fmt_id(&consumer.id)
    );
    Ok(())
}

// ── Targets ──────────────────────────────────────────────────────────────

pub async fn cmd_targets_list(
    client: &Client,
    service: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let targets: Vec<Target> = match &service {
        Some(s) => {
            let svc = resolve_service(client, s).await?;
            client.get(&format!("/services/{}/targets", svc.id)).await?
        }
        None => client.get("/targets").await?,
    };
    if fmt != OutputFormat::Table {
        return emit(&targets, fmt);
    }
    let by_id = service_name_map(client).await;
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|t| {
            vec![
                fmt_id(&t.id),
                by_id
                    .get(&t.service_id)
                    .cloned()
                    .unwrap_or_else(|| fmt_id(&t.service_id)),
                t.url.clone(),
                t.weight.to_string(),
                if t.enabled { "yes".into() } else { "no".into() },
            ]
        })
        .collect();
    print_table(&["ID", "SERVICE", "URL", "WEIGHT", "ENABLED"], &rows);
    Ok(())
}

pub async fn cmd_targets_add(
    client: &Client,
    service: String,
    url: String,
    weight: u32,
    fmt: OutputFormat,
) -> Result<()> {
    let svc = resolve_service(client, &service).await?;
    let body = serde_json::json!({ "url": url, "weight": weight, "enabled": true });
    let target: Target = client
        .post(&format!("/services/{}/targets", svc.id), &body)
        .await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} added target {} \u{2192} {}",
            green("\u{2713}"),
            target.url,
            svc.name
        );
    } else {
        emit(&target, fmt)?;
    }
    Ok(())
}

pub async fn cmd_targets_delete(client: &Client, id: &str) -> Result<()> {
    let uuid = Uuid::parse_str(id).context("target id must be a UUID")?;
    client.delete(&format!("/targets/{uuid}")).await?;
    println!("{} deleted target {}", green("\u{2713}"), fmt_id(&uuid));
    Ok(())
}

// ── Tokens ───────────────────────────────────────────────────────────────

pub async fn cmd_tokens_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let tokens: Vec<AdminTokenView> = client.get("/tokens").await?;
    if fmt != OutputFormat::Table {
        return emit(&tokens, fmt);
    }
    let rows: Vec<Vec<String>> = tokens
        .iter()
        .map(|t| {
            vec![
                fmt_id(&t.id),
                t.name.clone(),
                t.role.clone(),
                t.created_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                t.expires_at
                    .map(|e| e.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".into()),
                t.last_used_at
                    .map(|u| u.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    print_table(
        &["ID", "NAME", "ROLE", "CREATED", "EXPIRES", "LAST_USED"],
        &rows,
    );
    Ok(())
}

pub async fn cmd_tokens_create(
    client: &Client,
    name: String,
    role: String,
    expires_in_days: Option<u32>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut body = serde_json::json!({ "name": name, "role": role });
    if let Some(d) = expires_in_days {
        body["expires_in_days"] = Json::from(d);
    }
    let tok: AdminTokenView = client.post("/tokens", &body).await?;
    if fmt == OutputFormat::Table {
        println!(
            "{} created token {} ({}) role={}",
            green("\u{2713}"),
            tok.name,
            fmt_id(&tok.id),
            tok.role
        );
        if let Some(raw) = &tok.token {
            println!("{} {}", bold("token:"), raw);
            println!(
                "{}",
                dim("  (this is the only time the raw token is shown)")
            );
        }
    } else {
        emit(&tok, fmt)?;
    }
    Ok(())
}

pub async fn cmd_tokens_update(
    client: &Client,
    ident: &str,
    name: Option<String>,
    role: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let tokens: Vec<AdminTokenView> = client.get("/tokens").await?;
    let tok = resolve_by_name_or_id(&tokens, ident, |t| &t.name, |t| &t.id)
        .ok_or_else(|| anyhow!("no token matched '{ident}'"))?;
    let mut body = serde_json::Map::new();
    if let Some(n) = name {
        body.insert("name".into(), Json::String(n));
    }
    if let Some(r) = role {
        body.insert("role".into(), Json::String(r));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let updated: AdminTokenView = client
        .put(&format!("/tokens/{}", tok.id), &Json::Object(body))
        .await?;
    emit_success(
        &updated,
        fmt,
        "updated",
        "token",
        &updated.name,
        &updated.id,
    )
}

pub async fn cmd_tokens_delete(client: &Client, ident: &str) -> Result<()> {
    let tokens: Vec<AdminTokenView> = client.get("/tokens").await?;
    let tok = resolve_by_name_or_id(&tokens, ident, |t| &t.name, |t| &t.id)
        .ok_or_else(|| anyhow!("no token matched '{ident}'"))?;
    client.delete(&format!("/tokens/{}", tok.id)).await?;
    println!(
        "{} deleted token {} ({})",
        green("\u{2713}"),
        tok.name,
        fmt_id(&tok.id)
    );
    Ok(())
}

// ── Projects ─────────────────────────────────────────────────────────────

pub async fn cmd_projects_list(client: &Client, fmt: OutputFormat) -> Result<()> {
    let projects: Vec<Project> = client.get("/projects").await?;
    if fmt != OutputFormat::Table {
        return emit(&projects, fmt);
    }
    let rows: Vec<Vec<String>> = projects
        .iter()
        .map(|p| {
            vec![
                fmt_id(&p.id),
                p.name.clone(),
                p.description.chars().take(40).collect(),
                p.created_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "DESCRIPTION", "CREATED"], &rows);
    Ok(())
}

pub async fn cmd_projects_get(client: &Client, ident: &str, fmt: OutputFormat) -> Result<()> {
    let project = resolve_project(client, ident).await?;
    emit(&project, fmt)
}

pub async fn cmd_projects_create(
    client: &Client,
    name: String,
    description: String,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "name": name, "description": description });
    let project: Project = client.post("/projects", &body).await?;
    emit_success(
        &project,
        fmt,
        "created",
        "project",
        &project.name,
        &project.id,
    )
}

pub async fn cmd_projects_update(
    client: &Client,
    ident: &str,
    name: Option<String>,
    description: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let project = resolve_project(client, ident).await?;
    let mut body = serde_json::Map::new();
    if let Some(v) = name {
        body.insert("name".into(), Json::String(v));
    }
    if let Some(v) = description {
        body.insert("description".into(), Json::String(v));
    }
    if body.is_empty() {
        bail!("no fields supplied — nothing to update");
    }
    let updated: Project = client
        .patch_json(&format!("/projects/{}", project.id), &Json::Object(body))
        .await?;
    emit_success(
        &updated,
        fmt,
        "updated",
        "project",
        &updated.name,
        &updated.id,
    )
}

pub async fn cmd_projects_delete(client: &Client, ident: &str) -> Result<()> {
    let project = resolve_project(client, ident).await?;
    client.delete(&format!("/projects/{}", project.id)).await?;
    println!(
        "{} deleted project {} ({})",
        green("\u{2713}"),
        project.name,
        fmt_id(&project.id)
    );
    Ok(())
}

pub async fn cmd_projects_services(client: &Client, ident: &str, fmt: OutputFormat) -> Result<()> {
    let project = resolve_project(client, ident).await?;
    let services: Vec<Service> = client
        .get(&format!("/projects/{}/services", project.id))
        .await?;
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
                s.lb_strategy
                    .clone()
                    .unwrap_or_else(|| "round-robin".into()),
            ]
        })
        .collect();
    print_table(&["ID", "NAME", "URL", "LB"], &rows);
    Ok(())
}
