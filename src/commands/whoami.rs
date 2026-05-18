//! `lynxx whoami` — resolve the current billing API key and print the
//! account summary.
//!
//! Key resolution precedence (matches the rest of the CLI):
//!   1. `LYNXX_API_KEY` env var
//!   2. `[billing].api_key` from `~/.config/lynxx/config.toml`
//!
//! If neither is set, prints a helpful "Not signed in" message and exits 1.
//! With `--json`, prints the full `MeResp` pretty-printed instead of a
//! compact human-friendly table.

use anyhow::{Context, Result};

use crate::billing::{BillingClient, DEFAULT_BILLING_URL};
use crate::config::load_config;
use crate::output::bold;

pub struct WhoamiInput {
    /// Optional override for the billing URL.
    pub billing_url: Option<String>,
    /// API key from env (preferred over config).
    pub api_key_env: Option<String>,
    /// Output as pretty-printed JSON instead of the compact summary.
    pub json: bool,
}

pub async fn cmd_whoami(input: WhoamiInput) -> Result<()> {
    let cfg = load_config();

    // env > config for the API key.
    let api_key = input
        .api_key_env
        .clone()
        .or_else(|| cfg.billing.as_ref().and_then(|b| b.api_key.clone()));
    let Some(api_key) = api_key else {
        // Print to stderr so JSON consumers don't get polluted output.
        eprintln!("Not signed in. Run `lynxx login`.");
        // Documented contract: exits with nonzero status.
        std::process::exit(1);
    };

    // Billing URL: explicit override > config > default.
    let billing_url = input
        .billing_url
        .clone()
        .or_else(|| cfg.billing.as_ref().and_then(|b| b.url.clone()))
        .unwrap_or_else(|| DEFAULT_BILLING_URL.to_string());

    let client = BillingClient::new(&billing_url);
    let me = client.me(&api_key).await.context("fetching account info")?;

    if input.json {
        // Pretty JSON so it's pleasant to eyeball; still machine-parseable.
        println!(
            "{}",
            serde_json::to_string_pretty(&me).context("serialising MeResp")?,
        );
        return Ok(());
    }

    let nodes_cap = match me.max_nodes {
        Some(n) => n.to_string(),
        None => "unlimited".into(),
    };

    println!("{:<10}{}", bold("Customer:"), me.customer_urn);
    println!("{:<10}{}", bold("Email:"), me.email);
    println!("{:<10}{}", bold("Tier:"), me.tier);
    println!("{:<10}{} / {}", bold("Nodes:"), me.active_nodes, nodes_cap,);
    Ok(())
}
