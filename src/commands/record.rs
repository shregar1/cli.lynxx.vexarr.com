//! `lynxx record` — browse, show, replay, and delete request recordings.

use anyhow::Result;

use crate::cli::OutputFormat;
use crate::client::Client;
use crate::models::*;
use crate::output::{bold, dim, emit, green, print_table, red, truncate_for_diff, yellow};
use crate::util::{humanize_relative, parse_since, print_decoded_body, rec_short_id, urlencode};

pub async fn cmd_record_list(
    client: &Client,
    since: Option<String>,
    method: Option<String>,
    path_prefix: Option<String>,
    status: Option<u16>,
    limit: Option<usize>,
    fmt: OutputFormat,
) -> Result<()> {
    let mut qs: Vec<String> = Vec::new();
    if let Some(s) = since.as_deref() {
        let dt = parse_since(s)?;
        qs.push(format!("since={}", urlencode(&dt.to_rfc3339())));
    }
    if let Some(m) = method.as_deref() {
        qs.push(format!("method={}", urlencode(m)));
    }
    if let Some(p) = path_prefix.as_deref() {
        qs.push(format!("path_prefix={}", urlencode(p)));
    }
    if let Some(s) = status {
        qs.push(format!("status={s}"));
    }
    if let Some(l) = limit {
        qs.push(format!("limit={l}"));
    }
    let path = if qs.is_empty() {
        "/recordings".to_string()
    } else {
        format!("/recordings?{}", qs.join("&"))
    };
    let rows: Vec<RecordingIndexRow> = client.get(&path).await?;
    if fmt != OutputFormat::Table {
        return emit(&rows, fmt);
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                rec_short_id(&r.id),
                humanize_relative(r.ts),
                r.method.clone(),
                r.path.clone(),
                r.status.to_string(),
                format!("{}ms", r.duration_ms),
            ]
        })
        .collect();
    print_table(
        &["ID", "TS", "METHOD", "PATH", "STATUS", "DURATION"],
        &table_rows,
    );
    Ok(())
}

pub async fn cmd_record_show(client: &Client, id: &str, fmt: OutputFormat) -> Result<()> {
    let rec: Recording = client.get(&format!("/recordings/{id}")).await?;
    if fmt != OutputFormat::Table {
        return emit(&rec, fmt);
    }

    println!(
        "{} {} ({})",
        bold(&rec.request.method),
        rec.request.path,
        dim(&rec.id)
    );
    println!(
        "  {} {} \u{2192} status {} ({}ms)",
        dim("upstream"),
        if rec.upstream_url.is_empty() {
            "<unknown>"
        } else {
            rec.upstream_url.as_str()
        },
        rec.response.status,
        rec.duration_ms
    );
    if let Some(c) = rec.consumer.as_deref() {
        println!("  {} {}", dim("consumer"), c);
    }
    if let Some(s) = rec.service_id {
        println!("  {} {}", dim("service"), s);
    }
    if let Some(r) = rec.route_id {
        println!("  {} {}", dim("route"), r);
    }

    println!("\n{}", bold("Request headers"));
    for (k, v) in &rec.request.headers {
        println!("  {}: {}", dim(k), v);
    }
    if !rec.request.body_b64.is_empty() {
        println!("\n{}", bold("Request body"));
        print_decoded_body(&rec.request.body_b64);
    }

    println!("\n{}", bold("Response headers"));
    for (k, v) in &rec.response.headers {
        println!("  {}: {}", dim(k), v);
    }
    if !rec.response.body_b64.is_empty() {
        println!("\n{}", bold("Response body"));
        print_decoded_body(&rec.response.body_b64);
    }
    Ok(())
}

pub async fn cmd_record_replay(
    client: &Client,
    id: &str,
    against: Option<String>,
    fmt: OutputFormat,
) -> Result<()> {
    let body = serde_json::json!({ "target_url": against });
    let result: ReplayResult = client
        .post(&format!("/recordings/{id}/replay"), &body)
        .await?;
    if fmt != OutputFormat::Table {
        return emit(&result, fmt);
    }
    render_replay_diff(&result);
    Ok(())
}

pub fn render_replay_diff(r: &ReplayResult) {
    println!(
        "{} replay {} against {} ({}ms)",
        bold("\u{2192}"),
        rec_short_id(&r.original.id),
        r.target_url,
        r.elapsed_ms
    );
    match r.diff.status_changed {
        Some((before, after)) => {
            let line = format!("status: {before} \u{2192} {after}");
            println!("  {}", red(&line));
        }
        None => {
            println!("  {} status: {}", green("\u{2713}"), r.new_response.status);
        }
    }
    for (k, v) in &r.diff.headers_added {
        println!("  {} {}: {}", green("+"), k, truncate_for_diff(v));
    }
    for (k, v) in &r.diff.headers_removed {
        println!("  {} {}: {}", red("-"), k, truncate_for_diff(v));
    }
    for (k, before, after) in &r.diff.headers_changed {
        println!(
            "  {} {}: {} \u{2192} {}",
            yellow("~"),
            k,
            truncate_for_diff(before),
            truncate_for_diff(after)
        );
    }
    match &r.diff.body_diff {
        BodyDiff::Identical => {
            println!("  {} body identical", green("\u{2713}"));
        }
        BodyDiff::JsonDiff {
            added,
            removed,
            changed,
        } => {
            println!("  {}", bold("body (JSON):"));
            if let Some(obj) = added.as_object() {
                for (k, v) in obj {
                    println!(
                        "    {} {}: {}",
                        green("+"),
                        k,
                        truncate_for_diff(&v.to_string())
                    );
                }
            }
            if let Some(obj) = removed.as_object() {
                for (k, v) in obj {
                    println!(
                        "    {} {}: {}",
                        red("-"),
                        k,
                        truncate_for_diff(&v.to_string())
                    );
                }
            }
            if let Some(obj) = changed.as_object() {
                for (k, v) in obj {
                    let before = v.get("before").map(|x| x.to_string()).unwrap_or_default();
                    let after = v.get("after").map(|x| x.to_string()).unwrap_or_default();
                    println!(
                        "    {} {}: {} \u{2192} {}",
                        yellow("~"),
                        k,
                        truncate_for_diff(&before),
                        truncate_for_diff(&after)
                    );
                }
            }
        }
        BodyDiff::TextDiff { lines_changed } => {
            println!(
                "  {} body diff: {lines_changed} lines changed",
                yellow("~")
            );
        }
        BodyDiff::BinaryDiff {
            before_bytes,
            after_bytes,
        } => {
            println!(
                "  {} binary body: {before_bytes} \u{2192} {after_bytes} bytes",
                yellow("~")
            );
        }
    }
}

pub async fn cmd_record_delete(client: &Client, id: &str) -> Result<()> {
    client.delete(&format!("/recordings/{id}")).await?;
    println!(
        "{} deleted recording {}",
        green("\u{2713}"),
        rec_short_id(id)
    );
    Ok(())
}
