//! Entity resolution helpers — accept a name **or** UUID for human convenience,
//! with fuzzy "did you mean?" suggestions on miss.

use anyhow::{anyhow, Result};
use uuid::Uuid;

use crate::{fuzzy_suggest, Client, Consumer, Project, Service};

// ---------------------------------------------------------------------------
// Generic resolver — DRY backbone for resolve_service / resolve_consumer / etc.
// ---------------------------------------------------------------------------

/// Resolve an entity from a list by name, full UUID, or UUID prefix.
///
/// The `name_fn` / `id_fn` closures project the name and id fields out of `T`.
/// `kind` is used in error messages (e.g. "service", "consumer").
pub async fn resolve_by_field<T: Clone>(
    items: Vec<T>,
    ident: &str,
    name_fn: impl Fn(&T) -> &str,
    id_fn: impl Fn(&T) -> &Uuid,
    kind: &str,
) -> Result<T> {
    // 1) Exact UUID match.
    if let Ok(uuid) = Uuid::parse_str(ident) {
        if let Some(hit) = items.iter().find(|i| *id_fn(i) == uuid) {
            return Ok(hit.clone());
        }
    }

    // 2) Exact name match.
    if let Some(hit) = items.iter().find(|i| name_fn(i) == ident) {
        return Ok(hit.clone());
    }

    // 3) UUID prefix match.
    let prefix_matches: Vec<&T> = items
        .iter()
        .filter(|i| id_fn(i).to_string().starts_with(ident))
        .collect();

    match prefix_matches.len() {
        1 => Ok(prefix_matches[0].clone()),
        n if n > 1 => Err(anyhow!("'{ident}' is ambiguous ({n} matches)")),
        _ => {
            // 4) Fuzzy suggest on name.
            let suggest = fuzzy_suggest(ident, items.iter().map(name_fn));
            match suggest {
                Some(s) => Err(anyhow!("no {kind} matched '{ident}'. Did you mean: {s}?")),
                None => Err(anyhow!("no {kind} matched '{ident}'")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience wrappers that fetch + resolve in one call.
// ---------------------------------------------------------------------------

pub async fn resolve_service(client: &Client, ident: &str) -> Result<Service> {
    let services: Vec<Service> = client.get("/services").await?;
    resolve_by_field(services, ident, |s| s.name.as_str(), |s| &s.id, "service").await
}

pub async fn resolve_consumer(client: &Client, ident: &str) -> Result<Consumer> {
    let consumers: Vec<Consumer> = client.get("/consumers").await?;
    resolve_by_field(
        consumers,
        ident,
        |c| c.username.as_str(),
        |c| &c.id,
        "consumer",
    )
    .await
}

pub async fn resolve_project(client: &Client, ident: &str) -> Result<Project> {
    if let Ok(uuid) = Uuid::parse_str(ident) {
        return client.get(&format!("/projects/{uuid}")).await;
    }
    let projects: Vec<Project> = client.get("/projects").await?;
    projects
        .into_iter()
        .find(|p| p.name == ident)
        .ok_or_else(|| anyhow!("project not found: {ident}"))
}

/// Synchronous name-or-id lookup over a pre-fetched slice. Returns a
/// reference into the slice (no cloning).
pub fn resolve_by_name_or_id<'a, T>(
    items: &'a [T],
    ident: &str,
    name_fn: impl Fn(&T) -> &String,
    id_fn: impl Fn(&T) -> &Uuid,
) -> Option<&'a T> {
    if let Ok(uuid) = Uuid::parse_str(ident) {
        if let Some(it) = items.iter().find(|i| *id_fn(i) == uuid) {
            return Some(it);
        }
    }
    if let Some(it) = items.iter().find(|i| name_fn(i) == ident) {
        return Some(it);
    }
    items
        .iter()
        .find(|i| id_fn(i).to_string().starts_with(ident))
}
