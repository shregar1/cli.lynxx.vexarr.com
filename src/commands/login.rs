//! `lynxx login` — authenticate against the billing API and persist the key.
//!
//! Precedence for the API key input:
//!   1. `--api-key <key>` flag
//!   2. `LYNXX_API_KEY` env var
//!   3. interactive prompt on stdin (rpassword)
//!
//! On a successful `GET /v1/account/me`, the key (and any non-default
//! billing URL) is written into `~/.config/lynxx/config.toml` under
//! `[billing]`. The greeting is printed in the form
//!   `Signed in as <email> (tier: <tier>)`.
//!
//! On 401: clear "invalid API key" error and nonzero exit.
//! On 5xx: surfaced as a server-side error.

use anyhow::{bail, Context, Result};

use crate::billing::{BillingClient, DEFAULT_BILLING_URL};
use crate::config::{load_config, save_config, BillingConfig};
use crate::output::{bold, dim, green};

/// Resolved by `main.rs` from the `Cmd::Login(LoginArgs)` variant.
pub struct LoginInput {
    /// API key supplied via `--api-key`. May be `None`.
    pub api_key_flag: Option<String>,
    /// API key supplied via `LYNXX_API_KEY` env. May be `None`.
    pub api_key_env: Option<String>,
    /// Billing API base URL override (`--billing-url` or `LYNXX_BILLING_URL`).
    /// `None` means use the default.
    pub billing_url: Option<String>,
}

pub async fn cmd_login(input: LoginInput) -> Result<()> {
    // 1. Source the API key. Flag wins over env; if neither is set, prompt.
    let api_key = match input.api_key_flag.or(input.api_key_env) {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => prompt_api_key()?,
    };
    if api_key.is_empty() {
        bail!("no API key provided");
    }

    // 2. Resolve billing URL: explicit override or default.
    let billing_url = input
        .billing_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BILLING_URL.to_string());

    // 3. Hit /v1/account/me to validate the key.
    let client = BillingClient::new(&billing_url);
    let me = client.me(&api_key).await.context("authenticating")?;

    // 4. Persist the key into ~/.config/lynxx/config.toml under [billing].
    //    Only store the URL if the customer is using a non-default endpoint.
    let mut cfg = load_config();
    cfg.billing = Some(BillingConfig {
        api_key: Some(api_key.clone()),
        url: if billing_url.trim_end_matches('/') == DEFAULT_BILLING_URL {
            None
        } else {
            Some(billing_url.clone())
        },
    });
    let path = save_config(&cfg).context("writing config file")?;

    println!(
        "{} Signed in as {} (tier: {})",
        green("\u{2713}"),
        bold(&me.email),
        me.tier,
    );
    println!(
        "{}",
        dim(&format!("  credentials saved to {}", path.display()))
    );
    Ok(())
}

/// Prompt for an API key on stdin without echoing. Falls through with a
/// helpful error message when stdin is not a TTY (e.g. piped CI input).
fn prompt_api_key() -> Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        bail!(
            "no API key provided. pass --api-key, set LYNXX_API_KEY, \
             or run interactively to be prompted"
        );
    }
    let key =
        rpassword::prompt_password("Lynxx API key: ").context("reading API key from stdin")?;
    Ok(key.trim().to_string())
}
