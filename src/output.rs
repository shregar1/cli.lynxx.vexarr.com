//! Output formatting helpers: color, tables, fuzzy-matching, and emit.

use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::Serialize;
use uuid::Uuid;

use crate::OutputFormat;

// ---------------------------------------------------------------------------
// Color / TTY plumbing.
// ---------------------------------------------------------------------------

pub static COLOR_OFF: AtomicBool = AtomicBool::new(false);
pub static FULL_ID: AtomicBool = AtomicBool::new(false);

pub fn color_enabled() -> bool {
    !COLOR_OFF.load(Ordering::Relaxed) && io::stdout().is_terminal()
}
pub fn color_enabled_stderr() -> bool {
    !COLOR_OFF.load(Ordering::Relaxed) && io::stderr().is_terminal()
}

/// Common ANSI escape wrapper.  `stderr` selects which TTY check to use.
fn ansi(code: &str, s: &str, stderr: bool) -> String {
    if if stderr {
        color_enabled_stderr()
    } else {
        color_enabled()
    } {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    ansi("1", s, false)
}
pub fn dim(s: &str) -> String {
    ansi("2", s, false)
}
pub fn red(s: &str) -> String {
    ansi("31", s, true)
}
pub fn green(s: &str) -> String {
    ansi("32", s, false)
}
pub fn yellow(s: &str) -> String {
    ansi("33", s, false)
}

// ---------------------------------------------------------------------------
// Output helpers.
// ---------------------------------------------------------------------------

/// Print a monospace table. On TTY, uses light box-drawing chars (dim).
/// On non-TTY, falls back to plain pipe separators.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        eprintln!("{}", dim("(no rows)"));
        return;
    }
    let ncol = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(ncol) {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }

    let tty = color_enabled();
    let (h, v, tl, tr, bl, br, mt, mb, ml, mr, x) = if tty {
        ("─", "│", "┌", "┐", "└", "┘", "┬", "┴", "├", "┤", "┼")
    } else {
        ("-", "|", "+", "+", "+", "+", "+", "+", "+", "+", "+")
    };

    // Build a horizontal rule with given junctions.
    let rule = |left: &str, mid: &str, right: &str| {
        let mut out = String::from(left);
        for (i, w) in widths.iter().enumerate() {
            out.push_str(&h.repeat(w + 2));
            out.push_str(if i + 1 < ncol { mid } else { right });
        }
        out
    };

    println!("{}", dim(&rule(tl, mt, tr)));
    // Header row.
    let mut header_line = String::new();
    header_line.push_str(&dim(v));
    for (i, hd) in headers.iter().enumerate() {
        let pad = " ".repeat(widths[i].saturating_sub(hd.len()));
        header_line.push_str(&format!(" {}{} ", bold(hd), pad));
        header_line.push_str(&dim(v));
        let _ = i;
    }
    println!("{header_line}");
    println!("{}", dim(&rule(ml, x, mr)));
    // Body rows.
    for row in rows {
        let mut line = String::new();
        line.push_str(&dim(v));
        for (i, cell) in row.iter().enumerate().take(ncol) {
            let pad = " ".repeat(widths[i].saturating_sub(cell.len()));
            line.push_str(&format!(" {cell}{pad} "));
            line.push_str(&dim(v));
        }
        println!("{line}");
    }
    println!("{}", dim(&rule(bl, mb, br)));
}

/// Emit a value in the requested format. `Table` falls back to JSON.
pub fn emit<T: Serialize>(v: &T, fmt: OutputFormat) -> Result<()> {
    match fmt {
        OutputFormat::Json | OutputFormat::Table => {
            println!("{}", serde_json::to_string_pretty(v)?);
        }
        OutputFormat::Yaml => {
            print!("{}", serde_yaml::to_string(v)?);
        }
    }
    Ok(())
}

pub fn fmt_id(id: &Uuid) -> String {
    if FULL_ID.load(Ordering::Relaxed) {
        id.to_string()
    } else {
        id.to_string()[..8].to_string()
    }
}

// ---------------------------------------------------------------------------
// Levenshtein for "did you mean?" hints.
// ---------------------------------------------------------------------------

pub fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];
    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

pub fn fuzzy_suggest<'a>(
    needle: &str,
    haystack: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let n_lower = needle.to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for cand in haystack {
        let d = levenshtein(&n_lower, &cand.to_lowercase());
        let threshold = (cand.len() / 3).max(2);
        if d <= threshold && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, cand));
        }
    }
    best.map(|(_, s)| s.to_string())
}

pub fn pad(s: &str, w: usize) -> String {
    if s.len() >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - s.len()))
    }
}

pub fn truncate_for_diff(s: &str) -> String {
    if s.len() > 80 {
        format!("{}…", &s[..80])
    } else {
        s.to_string()
    }
}
