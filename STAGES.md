# cli.lynxx.vexarr.com — Build Stages

> **Product:** [lynxx](../) — Observability: logs, metrics, traces, dashboards.
> **Stack:** Rust + clap
> **Intent:** Terminal client for lynxx. First-class scripting + interactive flows.
>
> **How to use this file:** Every session starts by reading this checklist. Pick up where the last session left off. Never start coding without the corresponding stage doc. Never move to the next stage until the current stage's tests pass. Update status after completing each stage.

## Status Legend
- `[ ]` — Not started
- `[~]` — In progress
- `[x]` — Done (all tests pass, code merged)

---

> Last reviewed: 2026-05-18. Ticks reflect a code-walk audit of `src/`.

### Phase 1: Scaffold
- [x] 1.1 — Cargo project, `clap` derive macros, subcommand structure
- [x] 1.2 — Global flags: `--profile`, `--output {json|table|yaml}`, `--api-url`
- [x] 1.3 — Config loader: `~/.lynxx/config.toml`, env var overrides, flag overrides
- [x] 1.4 — Logging (tracing) with `--verbose` levels

### Phase 2: Authentication
- [~] 2.1 — `lynxx login` — paste API key flow (via `[billing]` config); device-code flow pending
- [x] 2.2 — `lynxx whoami` / `lynxx status` — show current profile
- [ ] 2.3 — `lynxx profile {add,use,list,remove}` — multi-profile support
- [ ] 2.4 — Secure key storage (OS keychain via `keyring` crate)

### Phase 3: Core commands
- [x] 3.1 — One subcommand per major API resource (see `src/commands/`)
- [x] 3.2 — `list / get / create / update / delete` per resource
- [ ] 3.3 — Pagination via `--page` and `--limit`
- [ ] 3.4 — Filters and search

### Phase 4: Output formatting
- [x] 4.1 — JSON output (default for scripting via `--output json`)
- [x] 4.2 — Table renderer (`src/output.rs`)
- [x] 4.3 — YAML output
- [ ] 4.4 — `--query` JSONPath / jq-style filter

### Phase 5: Interactive features
- [ ] 5.1 — `lynxx tail <resource>` — follow live events
- [~] 5.2 — Interactive prompts on missing required args (some commands)
- [~] 5.3 — Confirmation prompts on destructive actions (some commands)

### Phase 6: Tests + distribution
- [ ] 6.1 — Unit tests for arg parsing, integration tests against mock server (low coverage today)
- [~] 6.2 — `release-all-platforms.sh` matrix wrapper (replaces `cargo dist`)
- [ ] 6.3 — Homebrew formula, Scoop manifest, .deb / .rpm packages
- [ ] 6.4 — Auto-update check (opt-in)
- [ ] 6.5 — Shell completion (bash, zsh, fish, powershell)
