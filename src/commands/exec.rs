//! `lynxx exec` — fire a request through the gateway proxy.
//! `lynxx start` — exec the gateway binary.

use std::fs;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value as Json;

use crate::output::{bold, dim, green, red, yellow};
use crate::util::{locate_gateway, proxy_url_from_admin};

pub async fn cmd_exec(
    admin_url: &str,
    path: String,
    method: String,
    body: Option<String>,
    headers: Vec<String>,
    proxy_override: Option<String>,
    max_body: usize,
) -> Result<()> {
    let proxy_base = match proxy_override {
        Some(p) => p.trim_end_matches('/').to_string(),
        None => proxy_url_from_admin(admin_url)?,
    };
    let url = if path.starts_with('/') {
        format!("{proxy_base}{path}")
    } else {
        format!("{proxy_base}/{path}")
    };

    let m = method.to_ascii_uppercase();
    let method_parsed = reqwest::Method::from_bytes(m.as_bytes())
        .with_context(|| format!("invalid method '{m}'"))?;

    let body_bytes: Option<Vec<u8>> = match body {
        Some(s) if s.starts_with('@') => {
            let fpath = &s[1..];
            Some(fs::read(fpath).with_context(|| format!("reading body file '{fpath}'"))?)
        }
        Some(s) => Some(s.into_bytes()),
        None => None,
    };

    let http = reqwest::Client::new();
    let mut req = http.request(method_parsed.clone(), &url);
    for h in &headers {
        let (k, v) = h
            .split_once(':')
            .ok_or_else(|| anyhow!("header '{h}': expected 'Key: Value'"))?;
        req = req.header(k.trim(), v.trim());
    }
    if let Some(b) = body_bytes {
        req = req.body(b);
    }

    let started = Instant::now();
    let resp = req
        .send()
        .await
        .with_context(|| format!("{} {url}", method_parsed))?;
    let elapsed = started.elapsed();
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let raw = resp.bytes().await.unwrap_or_default();

    // Status line.
    let status_color = |s: reqwest::StatusCode| -> String {
        let n = s.as_u16();
        let s_str = format!("{} {}", n, s.canonical_reason().unwrap_or(""));
        match n {
            200..=299 => green(&s_str),
            300..=399 => yellow(&s_str),
            _ => red(&s_str),
        }
    };
    println!(
        "{} {}  {}  {}",
        bold(&method_parsed.to_string()),
        url,
        status_color(status),
        dim(&format!("({:.1}ms)", elapsed.as_secs_f64() * 1000.0)),
    );

    // Headers (dim).
    let mut hk: Vec<_> = resp_headers.iter().collect();
    hk.sort_by_key(|(k, _)| k.as_str().to_ascii_lowercase());
    for (k, v) in hk {
        let v_str = v.to_str().unwrap_or("<binary>");
        println!("{} {}: {}", dim("\u{2502}"), dim(k.as_str()), v_str);
    }
    println!();

    // Body.
    let content_type = resp_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let total_len = raw.len();
    let display: Vec<u8> = if max_body > 0 && total_len > max_body {
        raw[..max_body].to_vec()
    } else {
        raw.to_vec()
    };

    if content_type.contains("application/json") {
        match std::str::from_utf8(&display)
            .ok()
            .and_then(|s| serde_json::from_str::<Json>(s).ok())
        {
            Some(j) => {
                let pp = serde_json::to_string_pretty(&j).unwrap_or_default();
                println!("{pp}");
            }
            None => {
                println!("{}", String::from_utf8_lossy(&display));
            }
        }
    } else {
        println!("{}", String::from_utf8_lossy(&display));
    }
    if max_body > 0 && total_len > max_body {
        println!(
            "{}",
            dim(&format!(
                "(truncated: {} of {} bytes shown \u{2014} use --max-body 0 for full output)",
                max_body, total_len
            ))
        );
    }

    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    Ok(())
}

/// `lynxx start [...]` — replace the current process with `lynxx-gateway`.
///
/// `api_key` is the API key resolved by `main.rs` (precedence:
/// `--api-key` flag > `LYNXX_API_KEY` env > config). When present, we
/// re-export it into the spawned child's `LYNXX_API_KEY` env so a single
/// `lynxx login` (or a `lynxx --api-key … start …` invocation) covers
/// both the CLI auth check and the gateway runtime.
///
/// Without this re-export, only the env-var path would work — passing
/// `--api-key` via the CLI flag would never reach the gateway because
/// `exec()` would otherwise just inherit the unmodified parent env.
pub fn cmd_start(args: Vec<String>, api_key: Option<String>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let gateway = locate_gateway()?;
    println!("{} Starting Lynxx Gateway...", dim("\u{2192}"));
    println!(
        "{} Once started, the dashboard will be available at http://localhost:9001",
        green("\u{2713}")
    );

    let mut cmd = Command::new(&gateway);
    cmd.args(&args);
    // The child inherits the parent environment by default, so a present
    // LYNXX_API_KEY env already propagates. The remaining gap is the
    // global `--api-key` flag, which clap parsed into a local variable —
    // re-export it so both input paths reach the gateway.
    if let Some(key) = api_key.as_ref().filter(|k| !k.is_empty()) {
        cmd.env("LYNXX_API_KEY", key);
    }

    let err = cmd.exec();
    Err(anyhow::anyhow!(
        "failed to exec {}: {}",
        gateway.display(),
        err
    ))
}
