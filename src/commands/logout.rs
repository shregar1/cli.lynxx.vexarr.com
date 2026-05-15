//! `lynxx logout` — wipe the `[billing]` section from the local config.
//!
//! Idempotent: running on an unauthenticated machine is a no-op that still
//! prints `Signed out.` and exits 0. We don't try to revoke the key against
//! the billing API — local-only cleanup keeps the CLI usable offline.

use anyhow::Result;

use crate::config::{load_config, save_config};

pub fn cmd_logout() -> Result<()> {
    let mut cfg = load_config();
    if cfg.billing.is_some() {
        cfg.billing = None;
        save_config(&cfg)?;
    }
    println!("Signed out.");
    Ok(())
}
