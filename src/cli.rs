use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(
    name = "lynxx",
    version,
    about = "Declarative CLI for the Lynxx gateway",
    long_about = "lynxx is the official admin client for the Lynxx gateway. \
                  It speaks to the gateway's admin HTTP API and offers \
                  imperative CRUD, declarative apply/dump, live watch/tail, \
                  and a built-in route smoke-tester.",
    propagate_version = true
)]
pub struct Cli {
    /// Gateway admin API base URL (overrides config file)
    #[arg(long, env = "LYNXX_GATEWAY_BASE_URL", global = true)]
    pub gateway_url: Option<String>,

    /// API key for gateway authentication
    #[arg(long, env = "LYNXX_API_KEY", global = true)]
    pub api_key: Option<String>,

    /// Lynxx verification/licensing server base URL
    #[arg(long, env = "LYNXX_BASE_URL", global = true)]
    pub base_url: Option<String>,

    /// Lynxx billing API base URL (default: https://api.lynxx.vexarr.com)
    #[arg(long, env = "LYNXX_BILLING_URL", global = true)]
    pub billing_url: Option<String>,

    /// Lynxx license key
    #[arg(long, env = "LYNXX_LICENSE_KEY", global = true)]
    pub license_key: Option<String>,

    /// Lynxx secret key
    #[arg(long, env = "LYNXX_SECRET_KEY", global = true)]
    pub secret_key: Option<String>,

    /// Telemetry endpoint URL
    #[arg(long, env = "LYNXX_TELEMETRY_URL", global = true)]
    pub telemetry_url: Option<String>,

    /// Telemetry API key
    #[arg(long, env = "LYNXX_TELEMETRY_API_KEY", global = true)]
    pub telemetry_api_key: Option<String>,

    /// Output format for machine-readable commands
    #[arg(long, value_enum, global = true)]
    pub output: Option<OutputFormat>,

    /// Disable ANSI color output (also honors NO_COLOR env)
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Show full UUIDs in tables instead of 8-char prefixes
    #[arg(long, global = true)]
    pub full_id: bool,

    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Quick health probe of the admin API
    Ping,
    /// Dump /status as structured output
    Status,
    /// Manage projects
    #[command(subcommand)]
    Projects(ProjectsCmd),
    /// Manage services
    #[command(subcommand)]
    Services(ServicesCmd),
    /// Manage routes
    #[command(subcommand)]
    Routes(RoutesCmd),
    /// Manage plugins
    #[command(subcommand)]
    Plugins(PluginsCmd),
    /// Install / list / remove WASM plugins from a `.wasm` file
    #[command(subcommand)]
    Wasm(WasmCmd),
    /// Install / list / remove script-runner plugin instances
    #[command(subcommand)]
    Scripts(ScriptsCmd),
    /// Manage consumers
    #[command(subcommand)]
    Consumers(ConsumersCmd),
    /// Manage upstream targets
    #[command(subcommand)]
    Targets(TargetsCmd),
    /// Admin token management
    #[command(subcommand)]
    Tokens(TokensCmd),
    /// Audit log inspection
    #[command(subcommand)]
    Audit(AuditCmd),
    /// Circuit breaker management
    #[command(subcommand, alias = "cb")]
    CircuitBreakers(CircuitBreakersCmd),
    /// Query request logs
    #[command(subcommand)]
    Logs(LogsCmd),
    /// Zero-downtime deployment
    #[command(subcommand)]
    Deploy(DeployCmd),
    /// View quota usage snapshot
    Quota,
    /// Show upstream health across all services
    Upstreams,
    /// Dump raw Prometheus metrics
    Metrics,
    /// ACME certificate management
    #[command(subcommand)]
    Acme(AcmeCmd),
    /// Runtime profiling and debug info
    #[command(subcommand)]
    Debug(DebugCmd),
    /// Live gateway configuration management
    #[command(subcommand)]
    LiveConfig(LiveConfigCmd),
    /// Show per-service circuit/health summary
    Health,
    /// Trigger a config.toml reload on the gateway
    Reload,
    /// Dump full declarative state to stdout (YAML)
    Dump,
    /// Apply a declarative state file (upsert by name, delete missing)
    #[command(
        long_about = "Compute a Kubernetes-style diff plan against the current \
                            gateway state and apply it. Order: services → targets → \
                            routes → plugins, then deletes in reverse. Failures stop \
                            execution; partial application is NOT rolled back."
    )]
    Apply {
        /// Path to YAML state file ("-" for stdin)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        /// Print the diff plan but do not mutate (alias of plan-only)
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation (CI-friendly)
        #[arg(long)]
        auto_approve: bool,
    },
    /// Import an OpenAPI 3.x spec → services/routes/plugins (kubectl-style diff)
    #[command(
        long_about = "Read an OpenAPI 3.x document (file path or http(s) URL), \
                            translate it into Lynxx services + routes + (optionally) \
                            request-validator and openapi-mock plugins, then run the \
                            same diff/apply pipeline as `lynxx apply`. Re-importing \
                            the same spec is idempotent — names are derived \
                            deterministically from operationId / method+path so the \
                            second run produces an empty plan."
    )]
    Import {
        /// Path to the spec, or an http(s) URL
        spec: String,
        /// Prefix for generated service names (e.g. `--service-prefix=billing` → `billing-listInvoices`)
        #[arg(long)]
        service_prefix: Option<String>,
        /// Override the upstream URL (otherwise taken from spec's `servers[0].url`)
        #[arg(long)]
        upstream: Option<String>,
        /// Strip the spec's basePath off incoming URLs (sets `strip_path=true` on routes)
        #[arg(long)]
        strip_base_path: bool,
        /// Install request-validator plugins per route (default: on)
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        validators: bool,
        /// Install openapi-mock plugin per route, sourced from spec examples
        #[arg(long)]
        mock: bool,
        /// Print the diff plan but don't apply it
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation
        #[arg(long)]
        auto_approve: bool,
    },
    /// Validate a declarative state file without applying
    Validate {
        /// Path to YAML state file ("-" for stdin)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
    },
    /// Live-tail a list view; redraws every N seconds
    #[command(long_about = "Periodically re-renders the chosen list view in place. \
                            Resources: services, routes, plugins, health. Press Ctrl-C \
                            to exit; the cursor is always restored.")]
    Watch {
        /// Resource to watch
        resource: WatchResource,
        /// Refresh interval in seconds
        #[arg(short = 'n', long, default_value_t = 2)]
        interval: u64,
    },
    /// Live tail of /metrics, deriving per-tick request deltas
    Tail {
        /// Refresh interval in seconds
        #[arg(short = 'n', long, default_value_t = 1)]
        interval: u64,
    },
    /// Smoke-test a route through the gateway proxy
    Exec {
        /// Path on the gateway proxy (e.g. /api/v1/users)
        path: String,
        /// HTTP method
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        /// Request body. Prefix with @ to read from a file (e.g. @payload.json)
        #[arg(long)]
        body: Option<String>,
        /// Custom header (repeatable): "Key: Value"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// Override proxy base URL (default: admin host with port - 1)
        #[arg(long)]
        proxy: Option<String>,
        /// Body truncation limit in bytes (0 = no truncation)
        #[arg(long, default_value_t = 4096)]
        max_body: usize,
    },
    /// Browse + replay captured request/response pairs (Time-Travel Debug)
    #[command(subcommand)]
    Record(RecordCmd),
    /// Service analytics (traffic, latency, geo)
    #[command(subcommand)]
    Analytics(AnalyticsCmd),
    /// Health probes (synthetic checks)
    #[command(subcommand)]
    Probes(ProbesCmd),
    /// Configuration snapshots (backup / diff / rollback)
    #[command(subcommand)]
    Snapshots(SnapshotsCmd),
    /// Alert rules and events
    #[command(subcommand)]
    Alerts(AlertsCmd),
    /// IP allow/block rules
    #[command(subcommand, alias = "ip")]
    IpRules(IpRulesCmd),
    /// Custom error pages
    #[command(subcommand)]
    ErrorPages(ErrorPagesCmd),
    /// Consumer groups (rate-limit tiers)
    #[command(subcommand, alias = "cg")]
    ConsumerGroups(ConsumerGroupsCmd),
    /// Service dependency graph
    Graph {
        /// Service name or id
        service: String,
    },
    /// Auto-generated API documentation for a service
    #[command(subcommand)]
    Docs(DocsCmd),
    /// Replay an HTTP request through the gateway proxy
    ProxyReplay {
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        #[arg(long)]
        url: String,
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        #[arg(long)]
        body: Option<String>,
    },
    /// Manage local CLI config (~/.config/lynxx/config.toml)
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Generate shell completion script
    Completion {
        /// Target shell
        shell: Shell,
    },
    /// Run the Lynxx gateway (execs the lynxx-gateway binary in PATH).
    /// All extra arguments are forwarded.
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Start {
        /// Arguments forwarded to lynxx-gateway (e.g. --config /etc/lynxx.toml).
        args: Vec<String>,
    },
    /// Authenticate against the Lynxx billing API and persist the key
    Login(LoginArgs),
    /// Show the signed-in account from the billing API
    Whoami(WhoamiArgs),
    /// Forget the persisted billing API key
    Logout,
}

#[derive(clap::Args, Debug)]
pub struct LoginArgs {
    // Intentionally empty: the API key is read from the global `--api-key`
    // flag (which also honours `LYNXX_API_KEY`). If neither is set the
    // command prompts on stdin. Billing URL overrides come from the
    // global `--billing-url` flag / `LYNXX_BILLING_URL` env.
}

#[derive(clap::Args, Debug)]
pub struct WhoamiArgs {
    /// Print the raw billing API response as pretty JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum WatchResource {
    Services,
    Routes,
    Plugins,
    Health,
}

#[derive(Subcommand, Debug)]
pub enum ServicesCmd {
    List,
    Get {
        ident: String,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 3)]
        retries: u32,
        #[arg(long = "timeout", default_value_t = 5000)]
        connect_timeout_ms: u64,
        #[arg(long = "lb")]
        lb_strategy: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        subdomain: Option<String>,
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    /// Full update (PUT) — replaces all fields
    Update {
        /// Service name or id
        ident: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 3)]
        retries: u32,
        #[arg(long = "timeout", default_value_t = 5000)]
        connect_timeout_ms: u64,
        #[arg(long = "lb")]
        lb_strategy: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        subdomain: Option<String>,
    },
    /// Partial update (PATCH) — only supplied fields are changed
    Patch {
        /// Service name or id
        ident: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        retries: Option<u32>,
        #[arg(long = "timeout")]
        connect_timeout_ms: Option<u64>,
        #[arg(long = "lb")]
        lb_strategy: Option<String>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        subdomain: Option<String>,
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    Delete {
        ident: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RoutesCmd {
    List,
    /// Show a single route by id
    Get { id: String },
    Create {
        #[arg(long)]
        service: String,
        /// Comma-separated paths
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        /// Comma-separated methods
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        #[arg(long)]
        strip_path: bool,
    },
    /// Full update (PUT)
    Update {
        id: String,
        #[arg(long)]
        service: String,
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        #[arg(long)]
        strip_path: bool,
    },
    /// Partial update (PATCH) — only supplied fields are changed
    Patch {
        id: String,
        #[arg(long)]
        service: Option<String>,
        #[arg(long, value_delimiter = ',')]
        paths: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        methods: Option<Vec<String>>,
        #[arg(long)]
        strip_path: Option<bool>,
    },
    Delete {
        id: String,
    },
    /// Test a route through both the gateway and directly to the upstream
    Test {
        id: String,
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        #[arg(long, default_value = "/")]
        path: String,
        /// Custom header (repeatable): "Key: Value"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginsCmd {
    List,
    Install {
        /// Plugin name (e.g. jwt, rate-limit)
        name: String,
        /// Attach to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Attach to a route by id
        #[arg(long)]
        route: Option<String>,
        /// JSON config blob
        #[arg(long)]
        config: String,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WasmCmd {
    /// Install a `.wasm` plugin module
    Install {
        /// Path to the `.wasm` file
        path: PathBuf,
        /// Plugin instance name (default: file stem)
        #[arg(long)]
        name: Option<String>,
        /// JSON config blob forwarded to the guest via `lynxx_get_config`
        #[arg(long)]
        config: Option<String>,
        /// Scope to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Scope to a route by id
        #[arg(long)]
        route: Option<String>,
        /// wasmtime fuel budget per request
        #[arg(long, default_value_t = 10_000_000)]
        fuel: u64,
        /// Linear memory cap in kilobytes
        #[arg(long = "memory-kb", default_value_t = 16_384)]
        memory_kb: u32,
        /// Wall-clock budget per request, milliseconds
        #[arg(long = "wall-ms", default_value_t = 50)]
        wall_ms: u64,
    },
    /// List installed plugins whose backing impl is `wasm`
    List,
    /// Remove a wasm plugin instance by id (alias of `plugins delete`)
    Remove {
        /// Plugin instance id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScriptsCmd {
    /// Install a script-runner plugin instance
    Install {
        /// Command to run pre-request (e.g. `python3`)
        #[arg(long)]
        on_request_cmd: Option<String>,
        /// Arguments for the pre-request command (repeatable)
        #[arg(long = "on-request-args")]
        on_request_args: Vec<String>,
        /// Command to run post-response (e.g. `node`)
        #[arg(long)]
        on_response_cmd: Option<String>,
        /// Arguments for the post-response command (repeatable)
        #[arg(long = "on-response-args")]
        on_response_args: Vec<String>,
        /// Script execution timeout in milliseconds
        #[arg(long, default_value_t = 5000)]
        timeout: u64,
        /// Environment variables (repeatable, KEY=VALUE)
        #[arg(long = "env", value_parser = parse_key_val)]
        env: Vec<(String, String)>,
        /// Max body bytes passed to scripts
        #[arg(long, default_value_t = 2_097_152)]
        max_body_bytes: u64,
        /// Scope to a service by name
        #[arg(long)]
        service: Option<String>,
        /// Scope to a route by id
        #[arg(long)]
        route: Option<String>,
    },
    /// List installed script-runner plugin instances
    List,
    /// Remove a script-runner plugin instance by id
    Delete {
        /// Plugin instance id
        id: String,
    },
}

/// Parse a `KEY=VALUE` pair for `--env`.
pub fn parse_key_val(s: &str) -> std::result::Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

#[derive(Subcommand, Debug)]
pub enum ConsumersCmd {
    List,
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        custom_id: Option<String>,
    },
    Delete {
        ident: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TargetsCmd {
    List {
        #[arg(long)]
        service: Option<String>,
    },
    Add {
        #[arg(long)]
        service: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 1)]
        weight: u32,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TokensCmd {
    /// List all admin tokens
    List,
    /// Create a new admin token
    Create {
        #[arg(long)]
        name: String,
        /// Role: viewer, operator, admin
        #[arg(long)]
        role: String,
        /// Expiration in days from now
        #[arg(long)]
        expires_in_days: Option<u32>,
    },
    /// Update an existing token
    Update {
        /// Token id or name
        ident: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        role: Option<String>,
    },
    /// Revoke (delete) a token
    Delete {
        /// Token id or name
        ident: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// List audit log entries
    List {
        /// Filter by action (create, update, delete)
        #[arg(long)]
        action: Option<String>,
        /// Filter by resource type (service, route, plugin, consumer, token)
        #[arg(long = "type")]
        resource_type: Option<String>,
        /// Max entries to return
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Verify audit log hash chain integrity
    Verify,
}

#[derive(Subcommand, Debug)]
pub enum CircuitBreakersCmd {
    /// List all circuit breaker states
    List,
    /// Reset a service's circuit breaker to Closed
    Reset {
        /// Service name or id
        service: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectsCmd {
    List,
    Get { ident: String },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Partial update
    Update {
        ident: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    Delete { ident: String },
    /// List services belonging to a project
    Services { ident: String },
}

#[derive(Subcommand, Debug)]
pub enum LogsCmd {
    /// List recent request logs
    List {
        #[arg(long)]
        service_id: Option<Uuid>,
        #[arg(long)]
        route_id: Option<Uuid>,
        #[arg(long)]
        project_id: Option<Uuid>,
        #[arg(long)]
        level: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

#[derive(Subcommand, Debug)]
pub enum DeployCmd {
    /// Start a zero-downtime deploy for a service
    Start {
        /// Service name or id
        service: String,
        #[arg(long)]
        target_url: String,
        #[arg(long, default_value = "/health")]
        health_path: String,
        #[arg(long, default_value_t = 2000)]
        health_interval_ms: u64,
        #[arg(long, default_value_t = 3000)]
        health_timeout_ms: u64,
        #[arg(long, default_value_t = 3)]
        health_threshold: u32,
        #[arg(long, default_value_t = 30000)]
        drain_timeout_ms: u64,
        #[arg(long, default_value_t = 100)]
        weight: u32,
    },
    /// Check deploy status
    Status {
        /// Service name or id
        service: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AcmeCmd {
    /// Show ACME status
    Status,
    /// Trigger certificate renewal
    Renew,
    /// Revoke the current certificate
    Revoke,
    /// Set an HTTP-01 challenge response
    SetHttp01 {
        #[arg(long)]
        token: String,
        #[arg(long)]
        key_authorization: String,
    },
    /// Clear an HTTP-01 challenge
    ClearHttp01 {
        #[arg(long)]
        token: String,
    },
    /// Set a TLS-ALPN-01 challenge
    SetTlsAlpn01 {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        key_authorization: String,
    },
    /// Clear a TLS-ALPN-01 challenge
    ClearTlsAlpn01 {
        #[arg(long)]
        domain: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum DebugCmd {
    /// Heap memory profile
    Heap,
    /// Tokio tasks profile
    Tasks,
    /// Thread listing
    Threads,
    /// Process uptime and resource usage
    Uptime,
    /// Combined stats (heap + tasks + uptime)
    Stats,
}

#[derive(Subcommand, Debug)]
pub enum LiveConfigCmd {
    /// Show the current live configuration
    Get,
    /// Set a configuration value at a dotted path
    Set {
        /// Dotted config path (e.g. gateway.log_level)
        path: String,
        /// JSON value to set (e.g. '"debug"', '9000', 'true')
        value: String,
    },
    /// Validate a configuration body without applying
    Validate {
        /// JSON body to validate
        body: String,
    },
    /// Show configuration change history
    History,
    /// Lookup a service or route by its id tag (e.g. service:name or route:path)
    Lookup {
        /// Tag in the form "service:name" or "route:/path"
        tag: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecordCmd {
    /// List recent recordings (most recent first)
    List {
        /// Filter to recordings newer than this. Accepts relative
        /// (`30m`, `2h`, `1d`, `7d`) or absolute RFC 3339 timestamps.
        #[arg(long)]
        since: Option<String>,
        /// Filter by HTTP method (case-insensitive)
        #[arg(long)]
        method: Option<String>,
        /// Filter by request path prefix
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        /// Filter by response status code
        #[arg(long)]
        status: Option<u16>,
        /// Cap on returned rows (defaults to 100; max 1000)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show the full recording payload (request + response, headers, body)
    Show { id: String },
    /// Replay a recording and diff the response
    Replay {
        id: String,
        /// Replay against this URL. Defaults to the gateway proxy port.
        #[arg(long)]
        against: Option<String>,
    },
    /// Delete a recording (both the on-disk file and the index row)
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
pub enum AnalyticsCmd {
    /// Traffic metrics (requests, latency, errors)
    Traffic {
        /// Service name or id
        service: String,
        /// Time range: 1h, 6h, 24h, 7d, 30d
        #[arg(long, default_value = "24h")]
        range: String,
    },
    /// Rate-limit bucket data
    RateLimits {
        /// Service name or id
        service: String,
    },
    /// Aggregated rate-limit stats
    RateLimitStats {
        /// Service name or id
        service: String,
    },
    /// Geographic distribution of requests
    Geo {
        /// Service name or id
        service: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProbesCmd {
    /// List probes for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create a new health probe
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        route_id: Option<Uuid>,
        #[arg(long, default_value = "GET")]
        method: String,
        #[arg(long, default_value = "/")]
        path: String,
        #[arg(long, default_value_t = 60)]
        interval: i32,
        #[arg(long, default_value_t = 5000)]
        timeout: i32,
        #[arg(long, default_value_t = 200)]
        expected_status: i32,
    },
    /// Update a probe
    Patch {
        /// Probe id
        id: String,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        method: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        interval: Option<i32>,
        #[arg(long)]
        timeout: Option<i32>,
        #[arg(long)]
        expected_status: Option<i32>,
    },
    /// Delete a probe
    Delete { id: String },
    /// Show recent probe check results
    Results { id: String },
    /// Show probe uptime stats
    Stats { id: String },
}

#[derive(Subcommand, Debug)]
pub enum SnapshotsCmd {
    /// List snapshots for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create a configuration snapshot
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        name: String,
    },
    /// Diff a snapshot against current state
    Diff {
        /// Service name or id
        service: String,
        /// Snapshot id
        snapshot_id: String,
    },
    /// Rollback to a snapshot
    Rollback {
        /// Service name or id
        service: String,
        /// Snapshot id
        snapshot_id: String,
    },
    /// Delete a snapshot
    Delete {
        /// Service name or id
        service: String,
        /// Snapshot id
        snapshot_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AlertsCmd {
    /// List alert rules for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create an alert rule
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        metric: String,
        #[arg(long)]
        operator: String,
        #[arg(long)]
        threshold: f64,
        #[arg(long, default_value_t = 300)]
        window: i32,
        #[arg(long, default_value_t = 600)]
        cooldown: i32,
        /// JSON array of notification channels
        #[arg(long, default_value = "[]")]
        channels: String,
    },
    /// Update an alert rule
    Patch {
        /// Alert rule id
        id: String,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        threshold: Option<f64>,
        #[arg(long)]
        window: Option<i32>,
        #[arg(long)]
        cooldown: Option<i32>,
    },
    /// Delete an alert rule
    Delete { id: String },
    /// List alert events for a service
    Events {
        /// Service name or id
        service: String,
    },
    /// Resolve an alert event
    Resolve {
        /// Alert event id
        id: String,
    },
    /// Fire a test alert
    Test {
        /// Alert rule id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IpRulesCmd {
    /// List IP rules for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create an IP rule
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        cidr: String,
        /// allow or deny
        #[arg(long)]
        action: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        route_id: Option<Uuid>,
        #[arg(long, default_value_t = 0)]
        priority: i32,
    },
    /// Update an IP rule
    Patch {
        /// IP rule id
        id: String,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        cidr: Option<String>,
        #[arg(long)]
        action: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        priority: Option<i32>,
    },
    /// Delete an IP rule
    Delete { id: String },
    /// Bulk-create IP rules from a JSON file
    Bulk {
        /// Service name or id
        service: String,
        /// Path to JSON file with array of rules
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ErrorPagesCmd {
    /// List error pages for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create a custom error page
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        status_code: i32,
        #[arg(long, default_value = "text/html")]
        content_type: String,
        /// Body content (or @file to read from file)
        #[arg(long)]
        body: String,
    },
    /// Update an error page
    Patch {
        /// Error page id
        id: String,
        #[arg(long)]
        content_type: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Delete an error page
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
pub enum ConsumerGroupsCmd {
    /// List consumer groups for a service
    List {
        /// Service name or id
        service: String,
    },
    /// Create a consumer group
    Create {
        /// Service name or id
        service: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        rate_limit: Option<i32>,
        #[arg(long)]
        rate_window: Option<i32>,
        #[arg(long)]
        burst_limit: Option<i32>,
    },
    /// Update a consumer group
    Patch {
        /// Consumer group id
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        rate_limit: Option<i32>,
        #[arg(long)]
        rate_window: Option<i32>,
        #[arg(long)]
        burst_limit: Option<i32>,
    },
    /// Delete a consumer group
    Delete { id: String },
    /// List members in a consumer group
    Members { id: String },
    /// Add a consumer to a group
    AddMember {
        /// Consumer group id
        id: String,
        #[arg(long)]
        consumer_id: Uuid,
    },
    /// Remove a consumer from a group
    RemoveMember {
        /// Consumer group id
        id: String,
        /// Consumer id to remove
        consumer_id: String,
    },
    /// Show rate-limit usage for a consumer group
    Usage { id: String },
}

#[derive(Subcommand, Debug)]
pub enum DocsCmd {
    /// Show auto-generated API docs for a service
    Show {
        /// Service name or id
        service: String,
    },
    /// Try an HTTP request through the proxy
    Try {
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        #[arg(long)]
        url: String,
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        #[arg(long)]
        body: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Set a config key (e.g. `lynxx config set admin http://host:9001`)
    Set { key: String, value: String },
    /// Get a config key
    Get { key: String },
    /// Show the resolved config file
    Show,
    /// Print the config file path
    Path,
}
