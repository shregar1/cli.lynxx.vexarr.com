//! Miscellaneous helpers shared across the CLI.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value as Json;

use crate::{dim, COLOR_OFF, FULL_ID};

// ---------------------------------------------------------------------------
// Time formatting — moved to utilx::time, re-exported for existing call sites.
// ---------------------------------------------------------------------------

pub use utilx::time::{humanize_relative, humanize_secs, parse_since};

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

/// Query-value percent-encoder — moved to `utilx::encoding::url`.
pub use utilx::encoding::url::encode as urlencode;

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
