//! `lynxx watch` and `lynxx tail` — live monitoring commands.

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::output::{bold, dim, pad, red};
use crate::util::install_ctrlc_handler;

use std::sync::atomic::Ordering;

use super::admin::cmd_health;
use super::plugins::cmd_plugins_list;
use super::routes::cmd_routes_list;
use super::services::cmd_services_list;

/// Resource that `lynxx watch` can live-tail.
/// Re-exported here so dispatch can reference it.
pub use crate::cli::WatchResource;

// ---------------------------------------------------------------------------
// Watch
// ---------------------------------------------------------------------------

pub async fn cmd_watch(
    client: &Client,
    resource: WatchResource,
    interval: u64,
    fmt: OutputFormat,
) -> Result<()> {
    let stop = install_ctrlc_handler();
    let tty_out = io::stdout().is_terminal();

    if tty_out {
        print!("\x1b[?25l");
        let _ = io::stdout().flush();
    }

    let result: Result<()> = async {
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if tty_out {
                print!("\x1b[H\x1b[2J");
            }
            let now = Local::now().format("%Y-%m-%d %H:%M:%S");
            println!(
                "{} {} every {}s \u{2014} Ctrl-C to exit",
                bold(&format!("lynxx watch {:?}", resource).to_lowercase()),
                dim(&format!("[{now}]")),
                interval
            );
            println!();

            let render = match resource {
                WatchResource::Services => cmd_services_list(client, fmt).await,
                WatchResource::Routes => cmd_routes_list(client, fmt).await,
                WatchResource::Plugins => cmd_plugins_list(client, fmt).await,
                WatchResource::Health => cmd_health(client, fmt).await,
            };
            if let Err(e) = render {
                println!("{} {:#}", red("\u{2717}"), e);
            }
            let _ = io::stdout().flush();

            let total = Duration::from_secs(interval.max(1));
            let start = Instant::now();
            while start.elapsed() < total {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Ok(())
    }
    .await;

    if tty_out {
        print!("\x1b[?25h");
        let _ = io::stdout().flush();
    }
    result
}

// ---------------------------------------------------------------------------
// Tail — derive request deltas from /metrics each tick.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ReqKey {
    pub service: String,
    pub route: String,
    pub method: String,
    pub status: String,
}

/// Parse Prometheus text format.
pub fn parse_prom(text: &str) -> Vec<(String, Vec<(String, String)>, f64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, value) = match line.rsplit_once(' ') {
            Some(v) => v,
            None => continue,
        };
        let value: f64 = match value.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (name, labels) = if let Some(open) = head.find('{') {
            let close = match head.rfind('}') {
                Some(c) => c,
                None => continue,
            };
            let name = head[..open].to_string();
            let labels_str = &head[open + 1..close];
            let mut labels: Vec<(String, String)> = Vec::new();
            for kv in labels_str.split(',') {
                let kv = kv.trim();
                if kv.is_empty() {
                    continue;
                }
                let (k, v) = match kv.split_once('=') {
                    Some(s) => s,
                    None => continue,
                };
                let v = v.trim().trim_matches('"').to_string();
                labels.push((k.trim().to_string(), v));
            }
            labels.sort();
            (name, labels)
        } else {
            (head.to_string(), Vec::new())
        };
        out.push((name, labels, value));
    }
    out
}

pub fn status_bucket(s: &str) -> String {
    if let Ok(n) = s.parse::<u16>() {
        match n {
            100..=199 => "1xx".into(),
            200..=299 => "2xx".into(),
            300..=399 => "3xx".into(),
            400..=499 => "4xx".into(),
            500..=599 => "5xx".into(),
            _ => s.to_string(),
        }
    } else {
        s.to_string()
    }
}

pub async fn cmd_tail(client: &Client, interval: u64) -> Result<()> {
    let stop = install_ctrlc_handler();
    println!(
        "{} live request tail (interval {}s) \u{2014} best-effort, derived from /metrics counters",
        dim("note:"),
        interval
    );
    println!("{}", dim("press Ctrl-C to exit"));

    let mut prev: HashMap<ReqKey, f64> = HashMap::new();
    let mut prev_latency: HashMap<ReqKey, (f64, f64)> = HashMap::new();
    let mut first = true;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let text = match client.get_text("/metrics").await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{} {:#}", red("\u{2717}"), e);
                tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
                continue;
            }
        };
        let metrics = parse_prom(&text);

        let mut current: HashMap<ReqKey, f64> = HashMap::new();
        let mut current_latency: HashMap<ReqKey, (f64, f64)> = HashMap::new();
        for (name, labels, value) in &metrics {
            let lkup = |k: &str| {
                labels
                    .iter()
                    .find(|(lk, _)| lk == k)
                    .map(|(_, v)| v.clone())
            };
            let key = || ReqKey {
                service: lkup("service").unwrap_or_default(),
                route: lkup("route").unwrap_or_else(|| lkup("path").unwrap_or_default()),
                method: lkup("method").unwrap_or_default(),
                status: status_bucket(&lkup("status").unwrap_or_default()),
            };
            match name.as_str() {
                "lynxx_requests_total" => {
                    let k = key();
                    *current.entry(k).or_insert(0.0) += value;
                }
                "lynxx_request_duration_seconds_sum" => {
                    let entry = current_latency.entry(key()).or_insert((0.0, 0.0));
                    entry.0 += value;
                }
                "lynxx_request_duration_seconds_count" => {
                    let entry = current_latency.entry(key()).or_insert((0.0, 0.0));
                    entry.1 += value;
                }
                _ => {}
            }
        }

        if !first {
            let mut deltas: Vec<(ReqKey, f64, Option<f64>)> = Vec::new();
            for (k, &v) in &current {
                let p = prev.get(k).copied().unwrap_or(0.0);
                let d = v - p;
                if d > 0.0 {
                    let lat = match (current_latency.get(k), prev_latency.get(k)) {
                        (Some(&(s2, c2)), Some(&(s1, c1))) if c2 > c1 => {
                            Some(((s2 - s1) / (c2 - c1)) * 1000.0)
                        }
                        (Some(&(s2, c2)), None) if c2 > 0.0 => Some((s2 / c2) * 1000.0),
                        _ => None,
                    };
                    deltas.push((k.clone(), d, lat));
                }
            }
            deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let ts = Local::now().format("%H:%M:%S");
            for (k, count, lat) in &deltas {
                let lat_str = match lat {
                    Some(ms) => format!("{:.1}ms", ms),
                    None => "\u{2014}".into(),
                };
                let multiplier = if *count > 1.0 {
                    dim(&format!(" (\u{00d7} {:.0})", count))
                } else {
                    String::new()
                };
                println!(
                    "{} {} {} {} {} {}{}",
                    dim(&ts.to_string()),
                    pad(&k.service, 18),
                    pad(&k.method, 6),
                    pad(&k.route, 24),
                    pad(&k.status, 4),
                    lat_str,
                    multiplier,
                );
            }
        }

        prev = current;
        prev_latency = current_latency;
        first = false;

        let total = Duration::from_secs(interval.max(1));
        let start = Instant::now();
        while start.elapsed() < total {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}
