//! `lynxx dump`, `lynxx apply`, `lynxx validate` — declarative state management.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::client::Client;
use crate::models::*;
use crate::output::{dim, fmt_id, green, red, yellow};
use crate::util::confirm;

// ---------------------------------------------------------------------------
// Plan types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlanEntry {
    pub sigil: char,        // '+', '~', '-'
    pub kind: &'static str, // "service", "route", "plugin", "target"
    pub label: String,
    pub detail: Option<String>,
    /// Execution stage: 0=svc-upsert, 1=tgt, 2=route-upsert, 3=plugin-upsert,
    /// 4=plugin-delete, 5=target-delete, 6=route-delete, 7=svc-delete
    pub stage: u8,
    pub action: PlanAction,
}

#[derive(Debug, Clone)]
pub enum PlanAction {
    CreateService(DeclService),
    UpdateService {
        id: Uuid,
        ds: DeclService,
    },
    DeleteService {
        id: Uuid,
    },
    AddTarget {
        service_id: Uuid,
        url: String,
        weight: u32,
    },
    UpdateTarget {
        id: Uuid,
        weight: u32,
    },
    DeleteTarget {
        id: Uuid,
    },
    CreateRoute {
        service_name: String,
        paths: Vec<String>,
        methods: Vec<String>,
        strip_path: bool,
    },
    DeleteRoute {
        id: Uuid,
    },
    CreatePlugin {
        name: String,
        service: Option<String>,
        route_paths: Option<Vec<String>>,
        config: Json,
    },
    DeletePlugin {
        id: Uuid,
    },
}

// ---------------------------------------------------------------------------
// render_plan
// ---------------------------------------------------------------------------

pub fn render_plan(entries: &[PlanEntry]) {
    for e in entries {
        let sigil_str = e.sigil.to_string();
        let colored = match e.sigil {
            '+' => green(&sigil_str),
            '~' => yellow(&sigil_str),
            '-' => red(&sigil_str),
            _ => sigil_str.clone(),
        };
        let detail = match &e.detail {
            Some(d) => format!(" {}", dim(&format!("({d})"))),
            None => String::new(),
        };
        println!("{colored} {} {}{}", e.kind, e.label, detail);
    }
}

// ---------------------------------------------------------------------------
// cmd_dump
// ---------------------------------------------------------------------------

pub async fn cmd_dump(client: &Client) -> Result<()> {
    let services: Vec<Service> = client.get("/services").await?;
    let routes: Vec<Route> = client.get("/routes").await?;
    let plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let targets: Vec<Target> = client.get("/targets").await.unwrap_or_default();

    let svc_by_id: HashMap<Uuid, &Service> = services.iter().map(|s| (s.id, s)).collect();
    let route_by_id: HashMap<Uuid, &Route> = routes.iter().map(|r| (r.id, r)).collect();

    let mut targets_by_svc: HashMap<Uuid, Vec<DeclTarget>> = HashMap::new();
    for t in &targets {
        targets_by_svc
            .entry(t.service_id)
            .or_default()
            .push(DeclTarget {
                url: t.url.clone(),
                weight: t.weight,
            });
    }

    let decl_services: Vec<DeclService> = services
        .iter()
        .map(|s| DeclService {
            name: s.name.clone(),
            url: s.url.clone(),
            retries: s.retries,
            connect_timeout_ms: s.connect_timeout_ms,
            lb_strategy: s.lb_strategy.clone().unwrap_or_else(default_lb),
            targets: targets_by_svc.remove(&s.id).unwrap_or_default(),
        })
        .collect();

    let decl_routes: Vec<DeclRoute> = routes
        .iter()
        .map(|r| DeclRoute {
            service: svc_by_id
                .get(&r.service_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| r.service_id.to_string()),
            paths: r.paths.clone(),
            methods: r.methods.clone(),
            strip_path: r.strip_path,
        })
        .collect();

    let decl_plugins: Vec<DeclPlugin> = plugins
        .iter()
        .map(|p| {
            let (scope, service, route) = match (p.service_id, p.route_id) {
                (Some(sid), _) => (
                    "service".into(),
                    svc_by_id.get(&sid).map(|s| s.name.clone()),
                    None,
                ),
                (_, Some(rid)) => (
                    "route".into(),
                    None,
                    route_by_id.get(&rid).map(|r| r.paths.join(";")),
                ),
                _ => ("global".into(), None, None),
            };
            DeclPlugin {
                name: p.name.clone(),
                scope,
                service,
                route,
                config: p.config.clone(),
            }
        })
        .collect();

    let state = DeclarativeState {
        services: decl_services,
        routes: decl_routes,
        plugins: decl_plugins,
    };
    print!("{}", serde_yaml::to_string(&state)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_apply
// ---------------------------------------------------------------------------

pub async fn cmd_apply(
    client: &Client,
    file: PathBuf,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    let raw = if file == PathBuf::from("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?
    };
    let desired: DeclarativeState = serde_yaml::from_str(&raw).context("parsing state YAML")?;
    apply_state(client, desired, dry_run, auto_approve).await
}

/// Same body as `cmd_apply` but starting from an in-memory `DeclarativeState`.
/// Used by `lynxx import`.
pub async fn apply_state(
    client: &Client,
    desired: DeclarativeState,
    dry_run: bool,
    auto_approve: bool,
) -> Result<()> {
    // Snapshot current state.
    let cur_services: Vec<Service> = client.get("/services").await?;
    let cur_routes: Vec<Route> = client.get("/routes").await?;
    let cur_plugins: Vec<PluginInstance> = client.get("/plugins").await?;
    let cur_targets: Vec<Target> = client.get("/targets").await.unwrap_or_default();

    let svc_by_name: HashMap<String, &Service> =
        cur_services.iter().map(|s| (s.name.clone(), s)).collect();
    let svc_name_by_id: HashMap<Uuid, String> = cur_services
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    let want_svc_names: HashSet<&str> =
        desired.services.iter().map(|s| s.name.as_str()).collect();

    let mut plan: Vec<PlanEntry> = Vec::new();

    // ---- services upsert (stage 0)
    for ds in &desired.services {
        match svc_by_name.get(ds.name.as_str()) {
            Some(existing) => {
                let mut diffs: Vec<&'static str> = Vec::new();
                if existing.url != ds.url {
                    diffs.push("url");
                }
                if existing.retries != ds.retries {
                    diffs.push("retries");
                }
                if existing.connect_timeout_ms != ds.connect_timeout_ms {
                    diffs.push("timeout");
                }
                if existing.lb_strategy.as_deref().unwrap_or("round-robin") != ds.lb_strategy {
                    diffs.push("lb_strategy");
                }
                if !diffs.is_empty() {
                    plan.push(PlanEntry {
                        sigil: '~',
                        kind: "service",
                        label: ds.name.clone(),
                        detail: Some(format!("{} changed", diffs.join(", "))),
                        stage: 0,
                        action: PlanAction::UpdateService {
                            id: existing.id,
                            ds: ds.clone(),
                        },
                    });
                }
            }
            None => plan.push(PlanEntry {
                sigil: '+',
                kind: "service",
                label: ds.name.clone(),
                detail: Some(ds.url.clone()),
                stage: 0,
                action: PlanAction::CreateService(ds.clone()),
            }),
        }
    }

    // ---- targets diff (stage 1)
    let mut targets_by_svc: HashMap<Uuid, Vec<&Target>> = HashMap::new();
    for t in &cur_targets {
        targets_by_svc.entry(t.service_id).or_default().push(t);
    }
    for ds in &desired.services {
        let existing_svc_id = svc_by_name.get(ds.name.as_str()).map(|s| s.id);
        let have: BTreeMap<String, (Uuid, u32)> = existing_svc_id
            .and_then(|sid| targets_by_svc.get(&sid))
            .map(|v| {
                v.iter()
                    .map(|t| (t.url.clone(), (t.id, t.weight)))
                    .collect()
            })
            .unwrap_or_default();
        let want: BTreeMap<String, u32> = ds
            .targets
            .iter()
            .map(|t| (t.url.clone(), t.weight))
            .collect();

        for (url, weight) in &want {
            match have.get(url) {
                Some((_, w)) if w == weight => {}
                Some((tid, _)) => plan.push(PlanEntry {
                    sigil: '~',
                    kind: "target",
                    label: format!("{} ({})", url, ds.name),
                    detail: Some(format!("weight={weight}")),
                    stage: 1,
                    action: PlanAction::UpdateTarget {
                        id: *tid,
                        weight: *weight,
                    },
                }),
                None => {
                    let svc_id_marker = existing_svc_id.unwrap_or(Uuid::nil());
                    plan.push(PlanEntry {
                        sigil: '+',
                        kind: "target",
                        label: format!("{} ({})", url, ds.name),
                        detail: Some(format!("weight={weight}")),
                        stage: 1,
                        action: PlanAction::AddTarget {
                            service_id: svc_id_marker,
                            url: url.clone(),
                            weight: *weight,
                        },
                    });
                }
            }
        }
        for (url, (tid, _)) in &have {
            if !want.contains_key(url) {
                plan.push(PlanEntry {
                    sigil: '-',
                    kind: "target",
                    label: format!("{} ({})", url, ds.name),
                    detail: None,
                    stage: 5,
                    action: PlanAction::DeleteTarget { id: *tid },
                });
            }
        }
    }

    // ---- routes diff (stage 2 create / 6 delete)
    fn route_key(svc: &str, paths: &[String]) -> String {
        let mut p = paths.to_vec();
        p.sort();
        format!("{svc}|{}", p.join(","))
    }
    let cur_route_keys: HashMap<String, &Route> = cur_routes
        .iter()
        .map(|r| {
            let svc = svc_name_by_id
                .get(&r.service_id)
                .cloned()
                .unwrap_or_else(|| r.service_id.to_string());
            (route_key(&svc, &r.paths), r)
        })
        .collect();
    let want_route_keys: HashSet<String> = desired
        .routes
        .iter()
        .map(|r| route_key(&r.service, &r.paths))
        .collect();

    for dr in &desired.routes {
        let key = route_key(&dr.service, &dr.paths);
        if cur_route_keys.contains_key(&key) {
            continue;
        }
        plan.push(PlanEntry {
            sigil: '+',
            kind: "route",
            label: dr.paths.join(","),
            detail: Some(format!("\u{2192} {}", dr.service)),
            stage: 2,
            action: PlanAction::CreateRoute {
                service_name: dr.service.clone(),
                paths: dr.paths.clone(),
                methods: dr.methods.clone(),
                strip_path: dr.strip_path,
            },
        });
    }
    for (key, route) in &cur_route_keys {
        if !want_route_keys.contains(key) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "route",
                label: route.paths.join(","),
                detail: None,
                stage: 6,
                action: PlanAction::DeleteRoute { id: route.id },
            });
        }
    }

    // ---- plugins diff (stage 3 create / 4 delete)
    fn plugin_key(name: &str, scope: &str, target: &str) -> String {
        format!("{name}|{scope}|{target}")
    }
    let cur_plugin_keys: HashMap<String, &PluginInstance> = cur_plugins
        .iter()
        .map(|p| {
            let (scope, target) = match (p.service_id, p.route_id) {
                (Some(s), _) => (
                    "service",
                    svc_name_by_id
                        .get(&s)
                        .cloned()
                        .unwrap_or_else(|| s.to_string()),
                ),
                (_, Some(r)) => {
                    let k = cur_routes
                        .iter()
                        .find(|rt| rt.id == r)
                        .map(|rt| {
                            let svc = svc_name_by_id
                                .get(&rt.service_id)
                                .cloned()
                                .unwrap_or_else(|| rt.service_id.to_string());
                            let mut ps = rt.paths.clone();
                            ps.sort();
                            format!("{}|{}", svc, ps.join(";"))
                        })
                        .unwrap_or_else(|| r.to_string());
                    ("route", k)
                }
                _ => ("global", String::new()),
            };
            (plugin_key(&p.name, scope, &target), p)
        })
        .collect();

    let mut want_plugin_keys: HashSet<String> = Default::default();
    for dp in &desired.plugins {
        let (scope, target, route_paths_opt) = match dp.scope.as_str() {
            "service" => {
                let s = dp.service.as_deref().ok_or_else(|| {
                    anyhow!("plugin {} scope=service requires `service:`", dp.name)
                })?;
                ("service", s.to_string(), None)
            }
            "route" => {
                let r = dp.route.as_deref().ok_or_else(|| {
                    anyhow!(
                        "plugin {} scope=route requires `route:` (paths joined by ';')",
                        dp.name
                    )
                })?;
                let mut ps: Vec<String> = r.split(';').map(|s| s.to_string()).collect();
                ps.sort();
                let joined = ps.join(";");
                let svc = cur_routes
                    .iter()
                    .find(|rt| {
                        let mut got = rt.paths.clone();
                        got.sort();
                        got.join(";") == joined
                    })
                    .and_then(|rt| svc_name_by_id.get(&rt.service_id).cloned())
                    .unwrap_or_else(|| "?".into());
                ("route", format!("{svc}|{joined}"), Some(ps))
            }
            "global" => ("global", String::new(), None),
            other => bail!("unknown plugin scope '{other}'"),
        };
        let key = plugin_key(&dp.name, scope, &target);
        want_plugin_keys.insert(key.clone());

        if !cur_plugin_keys.contains_key(&key) {
            let scope_label = if target.is_empty() {
                scope.to_string()
            } else {
                format!("{scope} {target}")
            };
            plan.push(PlanEntry {
                sigil: '+',
                kind: "plugin",
                label: dp.name.clone(),
                detail: Some(scope_label),
                stage: 3,
                action: PlanAction::CreatePlugin {
                    name: dp.name.clone(),
                    service: if scope == "service" {
                        Some(target.clone())
                    } else {
                        None
                    },
                    route_paths: route_paths_opt,
                    config: dp.config.clone(),
                },
            });
        }
    }
    for (key, p) in &cur_plugin_keys {
        if !want_plugin_keys.contains(key) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "plugin",
                label: p.name.clone(),
                detail: Some(fmt_id(&p.id)),
                stage: 4,
                action: PlanAction::DeletePlugin { id: p.id },
            });
        }
    }

    // ---- service deletes (stage 7)
    for s in &cur_services {
        if !want_svc_names.contains(s.name.as_str()) {
            plan.push(PlanEntry {
                sigil: '-',
                kind: "service",
                label: s.name.clone(),
                detail: None,
                stage: 7,
                action: PlanAction::DeleteService { id: s.id },
            });
        }
    }

    // Sort by execution stage.
    plan.sort_by_key(|e| e.stage);

    if plan.is_empty() {
        println!("{} nothing to do", green("\u{2713}"));
        return Ok(());
    }

    render_plan(&plan);

    if dry_run {
        eprintln!(
            "{}",
            dim(&format!("(dry-run: {} change(s) not applied)", plan.len()))
        );
        return Ok(());
    }

    if !auto_approve {
        eprintln!();
        let ok = confirm(&format!("Apply {} change(s)? (y/N): ", plan.len()))?;
        if !ok {
            eprintln!("{}", dim("aborted."));
            return Ok(());
        }
    }

    // Re-snapshot service ids if we created any (stage 0).
    let mut svc_id_by_name: HashMap<String, Uuid> = cur_services
        .iter()
        .map(|s| (s.name.clone(), s.id))
        .collect();
    let mut route_id_by_key: HashMap<String, Uuid> = cur_routes
        .iter()
        .map(|r| {
            let svc = svc_name_by_id
                .get(&r.service_id)
                .cloned()
                .unwrap_or_else(|| r.service_id.to_string());
            let mut ps = r.paths.clone();
            ps.sort();
            (format!("{}|{}", svc, ps.join(";")), r.id)
        })
        .collect();

    let mut applied = 0usize;
    for entry in &plan {
        let res = apply_one(
            client,
            &entry.action,
            &mut svc_id_by_name,
            &mut route_id_by_key,
        )
        .await;
        match res {
            Ok(()) => {
                applied += 1;
                let s = entry.sigil.to_string();
                let colored = match entry.sigil {
                    '+' => green(&s),
                    '~' => yellow(&s),
                    '-' => red(&s),
                    _ => s.clone(),
                };
                println!("  {} {} {}", colored, dim("applied"), entry.label);
            }
            Err(e) => {
                eprintln!(
                    "{} step failed at stage {}: {} {}",
                    red("\u{2717}"),
                    entry.stage,
                    entry.kind,
                    entry.label
                );
                eprintln!("  {} {:#}", dim("\u{2514}\u{2500}"), e);
                eprintln!(
                    "{}",
                    dim(&format!(
                        "(stopped after {applied}/{} ops; partial state was NOT rolled back)",
                        plan.len()
                    ))
                );
                return Err(e);
            }
        }
    }

    println!("{} applied {} change(s)", green("\u{2713}"), applied);
    Ok(())
}

pub async fn apply_one(
    client: &Client,
    action: &PlanAction,
    svc_id_by_name: &mut HashMap<String, Uuid>,
    route_id_by_key: &mut HashMap<String, Uuid>,
) -> Result<()> {
    match action {
        PlanAction::CreateService(ds) => {
            let body = serde_json::json!({
                "name": ds.name, "url": ds.url, "retries": ds.retries,
                "connect_timeout_ms": ds.connect_timeout_ms,
                "lb_strategy": ds.lb_strategy,
            });
            let svc: Service = client.post("/services", &body).await?;
            svc_id_by_name.insert(svc.name.clone(), svc.id);
            Ok(())
        }
        PlanAction::UpdateService { id, ds } => {
            let body = serde_json::json!({
                "name": ds.name, "url": ds.url, "retries": ds.retries,
                "connect_timeout_ms": ds.connect_timeout_ms,
                "lb_strategy": ds.lb_strategy,
            });
            let _: Service = client.put(&format!("/services/{}", id), &body).await?;
            Ok(())
        }
        PlanAction::DeleteService { id } => client.delete(&format!("/services/{}", id)).await,
        PlanAction::AddTarget {
            service_id,
            url,
            weight,
        } => {
            if service_id.is_nil() {
                bail!("internal: target add missing service id");
            }
            let body = serde_json::json!({ "url": url, "weight": weight, "enabled": true });
            let _: Target = client
                .post(&format!("/services/{}/targets", service_id), &body)
                .await?;
            Ok(())
        }
        PlanAction::UpdateTarget { id, weight } => {
            let body = serde_json::json!({ "weight": weight });
            let _: Target = client.put(&format!("/targets/{}", id), &body).await?;
            Ok(())
        }
        PlanAction::DeleteTarget { id } => client.delete(&format!("/targets/{}", id)).await,
        PlanAction::CreateRoute {
            service_name,
            paths,
            methods,
            strip_path,
        } => {
            let svc_id = svc_id_by_name
                .get(service_name)
                .copied()
                .ok_or_else(|| anyhow!("route references unknown service '{service_name}'"))?;
            let body = serde_json::json!({
                "paths": paths, "methods": methods,
                "service_id": svc_id, "strip_path": strip_path,
            });
            let route: Route = client.post("/routes", &body).await?;
            let mut ps = route.paths.clone();
            ps.sort();
            route_id_by_key.insert(format!("{}|{}", service_name, ps.join(";")), route.id);
            Ok(())
        }
        PlanAction::DeleteRoute { id } => client.delete(&format!("/routes/{}", id)).await,
        PlanAction::CreatePlugin {
            name,
            service,
            route_paths,
            config,
        } => {
            let service_id = match service {
                Some(s) => Some(svc_id_by_name.get(s).copied().ok_or_else(|| {
                    anyhow!("plugin {name} references unknown service '{s}'")
                })?),
                None => None,
            };
            let route_id = match route_paths {
                Some(ps) => {
                    let mut ps2 = ps.clone();
                    ps2.sort();
                    let joined = ps2.join(";");
                    route_id_by_key.iter().find_map(|(k, v)| {
                        k.split_once('|').filter(|(_, p)| *p == joined).map(|_| *v)
                    })
                }
                None => None,
            };
            let body = serde_json::json!({
                "name": name, "service_id": service_id, "route_id": route_id,
                "enabled": true, "config": config,
            });
            let _: PluginInstance = client.post("/plugins", &body).await?;
            Ok(())
        }
        PlanAction::DeletePlugin { id } => client.delete(&format!("/plugins/{}", id)).await,
    }
}

// ---------------------------------------------------------------------------
// cmd_validate
// ---------------------------------------------------------------------------

pub async fn cmd_validate(client: &Client, file: PathBuf) -> Result<()> {
    let raw = if file == PathBuf::from("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?
    };
    let state: DeclarativeState = serde_yaml::from_str(&raw).context("parsing state YAML")?;

    let mut errors: Vec<String> = Vec::new();

    // services[*].url
    for (i, s) in state.services.iter().enumerate() {
        match url::Url::parse(&s.url) {
            Ok(u) => {
                if u.scheme().is_empty() {
                    errors.push(format!("services[{i}].url: invalid URL: missing scheme"));
                }
            }
            Err(e) => errors.push(format!("services[{i}].url ({}): invalid URL: {}", s.url, e)),
        }
        for (j, t) in s.targets.iter().enumerate() {
            if let Err(e) = url::Url::parse(&t.url) {
                errors.push(format!(
                    "services[{i}].targets[{j}].url ({}): invalid URL: {}",
                    t.url, e
                ));
            }
        }
        if s.name.is_empty() {
            errors.push(format!("services[{i}].name: must not be empty"));
        }
    }

    // routes[*].service exists in desired services or current gateway
    let want_svc: HashSet<&str> = state.services.iter().map(|s| s.name.as_str()).collect();
    let cur_services: Vec<Service> = client.get("/services").await.unwrap_or_default();
    let cur_svc_names: HashSet<String> = cur_services.iter().map(|s| s.name.clone()).collect();

    for (i, r) in state.routes.iter().enumerate() {
        if !want_svc.contains(r.service.as_str()) && !cur_svc_names.contains(&r.service) {
            use crate::output::fuzzy_suggest;
            let suggest = fuzzy_suggest(
                &r.service,
                want_svc
                    .iter()
                    .copied()
                    .chain(cur_svc_names.iter().map(|s| s.as_str())),
            );
            let hint = suggest
                .map(|s| format!(". Did you mean '{s}'?"))
                .unwrap_or_default();
            errors.push(format!(
                "routes[{i}].service ('{}'): no such service in state or gateway{hint}",
                r.service
            ));
        }
        if r.paths.is_empty() {
            errors.push(format!("routes[{i}].paths: must not be empty"));
        }
        for (j, p) in r.paths.iter().enumerate() {
            if !p.starts_with('/') {
                errors.push(format!(
                    "routes[{i}].paths[{j}]: path must start with '/' (got '{p}')"
                ));
            }
        }
    }

    // plugins[*].name in catalog
    for (i, p) in state.plugins.iter().enumerate() {
        if !PLUGIN_CATALOG.contains(&p.name.as_str()) {
            use crate::output::fuzzy_suggest;
            let suggest = fuzzy_suggest(&p.name, PLUGIN_CATALOG.iter().copied());
            let hint = suggest
                .map(|s| format!(". Did you mean '{s}'?"))
                .unwrap_or_default();
            errors.push(format!(
                "plugins[{i}].name ('{}'): unknown plugin{hint}",
                p.name
            ));
        }
        match &p.config {
            Json::Object(_) | Json::Null => {}
            _ => errors.push(format!(
                "plugins[{i}].config: must be a JSON object (got {})",
                match &p.config {
                    Json::String(_) => "string",
                    Json::Number(_) => "number",
                    Json::Bool(_) => "bool",
                    Json::Array(_) => "array",
                    _ => "scalar",
                }
            )),
        }
        match p.scope.as_str() {
            "global" => {}
            "service" => {
                if p.service.is_none() {
                    errors.push(format!("plugins[{i}]: scope=service requires `service:`"));
                } else if let Some(sn) = &p.service {
                    if !want_svc.contains(sn.as_str()) && !cur_svc_names.contains(sn) {
                        errors.push(format!(
                            "plugins[{i}].service ('{sn}'): no such service in state or gateway"
                        ));
                    }
                }
            }
            "route" => {
                if p.route.is_none() {
                    errors.push(format!(
                        "plugins[{i}]: scope=route requires `route:` (paths joined by ';')"
                    ));
                }
            }
            other => errors.push(format!("plugins[{i}].scope: unknown scope '{other}'")),
        }
    }

    if errors.is_empty() {
        let svc = state.services.len();
        let routes = state.routes.len();
        let plugins = state.plugins.len();
        println!(
            "{} valid: {svc} service(s), {routes} route(s), {plugins} plugin(s)",
            green("\u{2713}")
        );
        Ok(())
    } else {
        for e in &errors {
            println!("{} {}", red("\u{2717}"), e);
        }
        bail!("{} validation error(s)", errors.len());
    }
}
