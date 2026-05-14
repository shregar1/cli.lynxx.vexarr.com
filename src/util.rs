//! Miscellaneous helpers shared across the CLI.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::{dim, COLOR_OFF, FULL_ID};

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

/// Turn a duration in seconds into a compact human string: `42s`, `3m12s`,
/// `2h5m`, `1d4h`.
pub fn humanize_secs(s: i64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{}h", s / 86_400, (s % 86_400) / 3600)
    }
}

/// Produce a relative string like `12s ago`, `3m ago`, `2h ago`, `5d ago`.
pub fn humanize_relative(dt: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(dt);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "in future".into();
    }
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Parse a `--since` value: relative shorthand (`30m`, `2h`, `1d`, `7d`)
/// or absolute RFC 3339. Empty string is an error.
pub fn parse_since(raw: &str) -> Result<DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("--since: empty string");
    }
    // Try RFC 3339 first.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Relative: integer + unit.
    let (num_part, unit_part) =
        s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num_part
        .parse()
        .with_context(|| format!("--since: not a number: '{num_part}'"))?;
    let dur = match unit_part {
        "s" | "sec" | "secs" => chrono::Duration::seconds(n),
        "m" | "min" | "mins" => chrono::Duration::minutes(n),
        "h" | "hr" | "hrs" => chrono::Duration::hours(n),
        "d" | "day" | "days" => chrono::Duration::days(n),
        other => bail!("--since: unknown unit '{other}' (use s/m/h/d or RFC 3339)"),
    };
    Ok(Utc::now() - dur)
}

// ---------------------------------------------------------------------------
// ID formatting
// ---------------------------------------------------------------------------

/// Shorten a recording/opaque id to 8 chars unless `--full-id` was passed.
pub fn rec_short_id(id: &str) -> String {
    if FULL_ID.load(Ordering::Relaxed) {
        id.to_string()
    } else {
        id.chars().take(8).collect()
    }
}

// ---------------------------------------------------------------------------
// Body decoding
// ---------------------------------------------------------------------------

/// Decode a base64-encoded body, pretty-printing JSON if possible.
pub fn print_decoded_body(b64: &str) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    match STANDARD.decode(b64) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(s) => match serde_json::from_str::<Json>(s) {
                Ok(j) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&j).unwrap_or_else(|_| s.to_string())
                    );
                }
                Err(_) => {
                    println!("{s}");
                }
            },
            Err(_) => {
                println!("{}", dim(&format!("(binary, {} bytes)", bytes.len())));
            }
        },
        Err(e) => {
            println!("{}", dim(&format!("(undecodable: {e})")));
        }
    }
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Tiny URL-encoder for query-string values. Avoids pulling in `url` just
/// for this; we only need to escape a handful of bytes.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Derive the gateway proxy URL from the admin URL by decrementing the port
/// (admin 9001 -> proxy 9000). Falls back to port 9000 when no explicit port
/// is given.
pub fn proxy_url_from_admin(admin: &str) -> Result<String> {
    let mut u = url::Url::parse(admin).context("parsing --admin URL")?;
    if let Some(p) = u.port() {
        let proxy_port = p
            .checked_sub(1)
            .ok_or_else(|| anyhow!("admin port {p} cannot be decremented to derive proxy port"))?;
        u.set_port(Some(proxy_port))
            .map_err(|_| anyhow!("setting proxy port"))?;
    } else {
        // Default convention: admin 9001 -> proxy 9000.
        u.set_port(Some(9000))
            .map_err(|_| anyhow!("setting proxy port"))?;
    }
    u.set_path("");
    Ok(u.as_str().trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------------------
// Interactive
// ---------------------------------------------------------------------------

/// Ask a y/N confirmation on stderr. Returns `Ok(true)` if the user typed
/// `y` or `yes`, `Ok(false)` otherwise. Bails if stdin is not a TTY.
pub fn confirm(prompt: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        // No TTY -> safest to bail out and require --auto-approve
        bail!("non-interactive shell; pass --auto-approve to confirm");
    }
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let s = buf.trim().to_ascii_lowercase();
    Ok(s == "y" || s == "yes")
}

// ---------------------------------------------------------------------------
// Ctrl-C
// ---------------------------------------------------------------------------

/// Install a Ctrl-C handler that sets the returned flag to `true`.
pub fn install_ctrlc_handler() -> Arc<std::sync::atomic::AtomicBool> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = stop.clone();
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            s.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    stop
}

// ---------------------------------------------------------------------------
// Gateway discovery
// ---------------------------------------------------------------------------

/// Locate the `lynxx-gateway` binary next to the current executable or on
/// `$PATH`.
pub fn locate_gateway() -> Result<PathBuf> {
    if let Ok(self_path) = std::env::current_exe() {
        if let Some(dir) = self_path.parent() {
            let candidate = dir.join("lynxx-gateway");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("lynxx-gateway");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(anyhow!(
        "lynxx-gateway binary not found next to lynxx or on PATH. \
         Reinstall via `curl -fsSL https://api.lynxx.vexarr.com/install.sh | bash`."
    ))
}

// ---------------------------------------------------------------------------
// Error display
// ---------------------------------------------------------------------------

/// Print a formatted error chain to stderr. Uses ANSI color when the terminal
/// supports it.
pub fn print_error(e: &anyhow::Error) {
    let stderr_tty = io::stderr().is_terminal();
    let prefix = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
        "\x1b[31m\x1b[1m\u{2717}\x1b[0m "
    } else {
        "\u{2717} "
    };

    // Pull the head error as the "summary" line; show the chain (if any) as
    // dim continuation.
    let head = e.to_string();
    let mut chain: Vec<String> = Vec::new();
    let mut src = e.source();
    while let Some(s) = src {
        chain.push(s.to_string());
        src = s.source();
    }

    // ApiError's Display already includes the parsed envelope message; show
    // that as the one-liner.
    let _ = writeln!(io::stderr(), "{prefix}{head}");
    if !chain.is_empty() {
        let cont = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
            "\x1b[2m\u{2514}\u{2500}\x1b[0m"
        } else {
            "\u{2514}\u{2500}"
        };
        for c in &chain {
            let line = if stderr_tty && !COLOR_OFF.load(Ordering::Relaxed) {
                format!("\x1b[2m{c}\x1b[0m")
            } else {
                c.clone()
            };
            let _ = writeln!(io::stderr(), "  {cont} {line}");
        }
    }
}
