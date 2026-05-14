//! OpenAPI 3.x import — translates an OpenAPI spec into a `DeclarativeState`
//! that can be fed into the `apply` pipeline.
//!
//! Translation contract (deliberately simple, deliberately deterministic so
//! re-imports produce zero churn):
//!
//! - One Lynxx service per unique upstream URL.
//! - One Lynxx route per (operation, path-template).
//! - Service name: `<prefix>-<slug>` where slug is the spec's `operationId`
//!   if present, else `<method>-<path>` slugified.
//! - `request-validator` plugin when the operation has a requestBody with an
//!   `application/json` schema. (`--validators=false` to skip.)
//! - `openapi-mock` plugin per route when `--mock` is set.

use anyhow::{anyhow, Result};

use crate::{
    DeclPlugin, DeclRoute, DeclService, DeclarativeState, default_lb, default_retries,
    default_timeout,
};

// ---------------------------------------------------------------------------
// Core conversion
// ---------------------------------------------------------------------------

/// Convert a parsed OpenAPI doc into the gateway's `DeclarativeState`.
///
/// We surface bad input as `anyhow` errors with enough context for an
/// operator to fix the spec; we do **not** silently drop operations.
#[allow(clippy::too_many_arguments)]
pub fn openapi_to_state(
    spec: &openapiv3::OpenAPI,
    spec_source: &str,
    service_prefix: Option<&str>,
    upstream_override: Option<&str>,
    strip_base_path: bool,
    install_validators: bool,
    install_mock: bool,
) -> Result<DeclarativeState> {
    use openapiv3::ReferenceOr;

    let upstream_default = upstream_override
        .map(|s| s.to_string())
        .or_else(|| spec.servers.first().map(|s| s.url.clone()))
        .ok_or_else(|| {
            anyhow!("no upstream URL: pass --upstream or add `servers[0].url` to the spec")
        })?;

    // One service per unique upstream URL. We keep insertion order stable
    // so re-imports of the same spec produce the same service names.
    let mut services_by_url: indexed::IndexMap<String, String> = indexed::IndexMap::new();
    let mut routes: Vec<DeclRoute> = Vec::new();
    let mut plugins: Vec<DeclPlugin> = Vec::new();

    for (path_tmpl, item_ref) in &spec.paths.paths {
        let item = match item_ref {
            ReferenceOr::Item(it) => it,
            ReferenceOr::Reference { .. } => continue,
        };

        let methods: [(&str, &Option<openapiv3::Operation>); 8] = [
            ("GET", &item.get),
            ("POST", &item.post),
            ("PUT", &item.put),
            ("DELETE", &item.delete),
            ("PATCH", &item.patch),
            ("HEAD", &item.head),
            ("OPTIONS", &item.options),
            ("TRACE", &item.trace),
        ];

        for (method, op_opt) in methods {
            let op = match op_opt {
                Some(o) => o,
                None => continue,
            };

            // Service name slug: prefix + (operationId | method-path).
            let slug = match &op.operation_id {
                Some(id) => slugify(id),
                None => slugify(&format!("{}-{}", method.to_lowercase(), path_tmpl)),
            };
            let service_name = match service_prefix {
                Some(p) => format!("{}-{}", slugify(p), slug),
                None => slug.clone(),
            };

            // Route path: `{name}` -> `*`.
            let route_path = openapi_path_to_lynxx(path_tmpl);

            // Reuse an existing service for this upstream URL; otherwise create
            // one and register the slug we derived.
            let svc_name_final = services_by_url
                .entry(upstream_default.clone())
                .or_insert_with(|| service_name.clone())
                .clone();

            routes.push(DeclRoute {
                service: svc_name_final.clone(),
                paths: vec![route_path.clone()],
                methods: vec![method.to_string()],
                strip_path: strip_base_path,
            });

            // request-validator plugin.
            if install_validators {
                if let Some(schema) = first_request_body_schema(op) {
                    plugins.push(DeclPlugin {
                        name: "request-validator".into(),
                        scope: "route".into(),
                        service: None,
                        route: Some(route_path.clone()),
                        config: serde_json::json!({ "body_schema": schema }),
                    });
                }
            }

            // openapi-mock plugin.
            if install_mock {
                plugins.push(DeclPlugin {
                    name: "openapi-mock".into(),
                    scope: "route".into(),
                    service: None,
                    route: Some(route_path.clone()),
                    config: serde_json::json!({
                        "spec_path": spec_source,
                        "default_status": 200,
                        "delay_ms": 0,
                        "passthrough_unmatched": false,
                    }),
                });
            }
        }
    }

    let services: Vec<DeclService> = services_by_url
        .iter()
        .map(|(url, name)| DeclService {
            name: name.clone(),
            url: url.clone(),
            retries: default_retries(),
            connect_timeout_ms: default_timeout(),
            lb_strategy: default_lb(),
            targets: Vec::new(),
        })
        .collect();

    Ok(DeclarativeState {
        services,
        routes,
        plugins,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `/users/{id}/posts/{pid}` -> `/users/*/posts/*`
pub fn openapi_path_to_lynxx(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut in_param = false;
    for ch in p.chars() {
        match ch {
            '{' => {
                in_param = true;
                out.push('*');
            }
            '}' => in_param = false,
            _ if in_param => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Slugify a name into something safe as a Lynxx service name. The
/// slugification is deterministic so re-imports of the same spec produce the
/// same names -- that is the entire idempotency story.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("op");
    }
    out
}

/// Pull the request-body schema for `application/json` (or the first content
/// type if there is no JSON one) and return it as raw JSON. Returns `None` if
/// the operation has no requestBody or its schema is a `$ref` we cannot
/// dereference.
pub fn first_request_body_schema(op: &openapiv3::Operation) -> Option<serde_json::Value> {
    use openapiv3::ReferenceOr;
    let rb_ref = op.request_body.as_ref()?;
    let rb = match rb_ref {
        ReferenceOr::Item(b) => b,
        ReferenceOr::Reference { .. } => return None,
    };
    let media = rb
        .content
        .get("application/json")
        .or_else(|| rb.content.iter().next().map(|(_, v)| v))?;
    let schema_ref = media.schema.as_ref()?;
    serde_json::to_value(schema_ref).ok()
}

// ---------------------------------------------------------------------------
// Tiny insertion-order map (avoids an `indexmap` dependency for one site).
// ---------------------------------------------------------------------------

pub mod indexed {
    pub struct IndexMap<K, V> {
        keys: Vec<K>,
        values: Vec<V>,
    }
    impl<K: PartialEq + Clone, V: Clone> IndexMap<K, V> {
        pub fn new() -> Self {
            Self {
                keys: Vec::new(),
                values: Vec::new(),
            }
        }
        pub fn entry(&mut self, k: K) -> Entry<'_, K, V> {
            let idx = self.keys.iter().position(|x| x == &k);
            Entry {
                map: self,
                key: k,
                idx,
            }
        }
        pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
            self.keys.iter().zip(self.values.iter())
        }
    }
    pub struct Entry<'a, K, V> {
        map: &'a mut IndexMap<K, V>,
        key: K,
        idx: Option<usize>,
    }
    impl<'a, K: Clone, V: Clone> Entry<'a, K, V> {
        pub fn or_insert_with<F: FnOnce() -> V>(self, f: F) -> &'a mut V {
            match self.idx {
                Some(i) => &mut self.map.values[i],
                None => {
                    self.map.keys.push(self.key);
                    self.map.values.push(f());
                    self.map.values.last_mut().unwrap()
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod openapi_import_tests {
    use super::*;

    fn spec() -> openapiv3::OpenAPI {
        let raw = r#"
openapi: 3.0.3
info: { title: t, version: '1.0' }
servers:
  - url: https://api.example.com
paths:
  /users:
    get:
      operationId: listUsers
      responses: { '200': { description: ok } }
    post:
      operationId: createUser
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [email]
              properties: { email: { type: string } }
      responses: { '201': { description: created } }
  /users/{id}:
    get:
      operationId: getUser
      responses: { '200': { description: ok } }
"#;
        serde_yaml::from_str(raw).unwrap()
    }

    #[test]
    fn produces_one_service_per_upstream() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        assert_eq!(s.services.len(), 1);
        assert_eq!(s.services[0].url, "https://api.example.com");
    }

    #[test]
    fn produces_one_route_per_operation() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        assert_eq!(
            s.routes.len(),
            3,
            "GET /users, POST /users, GET /users/{{id}}"
        );
        assert!(s
            .routes
            .iter()
            .any(|r| r.paths == vec!["/users/*"] && r.methods == vec!["GET"]));
    }

    #[test]
    fn installs_validator_only_for_request_body_routes() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, true, false).unwrap();
        let validator_count = s
            .plugins
            .iter()
            .filter(|p| p.name == "request-validator")
            .count();
        assert_eq!(validator_count, 1);
    }

    #[test]
    fn idempotent_reimport_produces_same_state() {
        let a = openapi_to_state(
            &spec(),
            "spec.yaml",
            Some("billing"),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        let b = openapi_to_state(
            &spec(),
            "spec.yaml",
            Some("billing"),
            None,
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            serde_yaml::to_string(&a).unwrap(),
            serde_yaml::to_string(&b).unwrap()
        );
    }

    #[test]
    fn mock_flag_adds_mock_plugin_per_route() {
        let s = openapi_to_state(&spec(), "spec.yaml", None, None, false, false, true).unwrap();
        let mock_count = s
            .plugins
            .iter()
            .filter(|p| p.name == "openapi-mock")
            .count();
        assert_eq!(mock_count, 3);
    }

    #[test]
    fn upstream_override_wins() {
        let s = openapi_to_state(
            &spec(),
            "spec.yaml",
            None,
            Some("https://override"),
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(s.services[0].url, "https://override");
    }

    #[test]
    fn path_param_to_wildcard() {
        assert_eq!(openapi_path_to_lynxx("/users/{id}"), "/users/*");
        assert_eq!(openapi_path_to_lynxx("/a/{x}/b/{y}"), "/a/*/b/*");
        assert_eq!(openapi_path_to_lynxx("/static"), "/static");
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(slugify("listUsers"), "listusers");
        assert_eq!(slugify("get-user_v2"), "get-user-v2");
        assert_eq!(slugify("/users/{id}"), "users-id");
    }
}
