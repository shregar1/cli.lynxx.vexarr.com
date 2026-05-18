use std::fs;
use std::path::Path;

use anyhow::{bail, Result};

use crate::output::green;

const TEMPLATE: &str = r#"[gateway]
proxy_port = 8000
admin_port = 8001

[license]
# Set via LYNXX_API_KEY env var or paste here.
# Get a key at https://lynxx.vexarr.com/account/api-keys
# api_key = ""

# [[services]]
# name = "my-api"
# url = "http://127.0.0.1:3000"
# retries = 3
# connect_timeout_ms = 5000

# [[routes]]
# service = "my-api"
# paths = ["/api/v1"]
# methods = ["GET", "POST"]
# strip_path = false
"#;

pub fn cmd_init(force: bool) -> Result<()> {
    cmd_init_in(Path::new("."), force)
}

fn cmd_init_in(dir: &Path, force: bool) -> Result<()> {
    let path = dir.join("lynxx.toml");

    if path.exists() && !force {
        bail!(
            "lynxx.toml already exists. Use --force to overwrite."
        );
    }

    fs::write(&path, TEMPLATE)?;
    println!("{} created lynxx.toml", green("✓"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_file() {
        let dir = TempDir::new().unwrap();
        cmd_init_in(dir.path(), false).unwrap();

        let content = fs::read_to_string(dir.path().join("lynxx.toml")).unwrap();
        assert!(content.contains("[gateway]"));
        assert!(content.contains("proxy_port = 8000"));
        assert!(content.contains("admin_port = 8001"));
        assert!(content.contains("[license]"));
        assert!(content.contains("# [[services]]"));
    }

    #[test]
    fn refuses_overwrite_without_force() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("lynxx.toml"), "existing").unwrap();

        let err = cmd_init_in(dir.path(), false);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already exists"));

        let content = fs::read_to_string(dir.path().join("lynxx.toml")).unwrap();
        assert_eq!(content, "existing");
    }

    #[test]
    fn force_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("lynxx.toml"), "old").unwrap();

        cmd_init_in(dir.path(), true).unwrap();

        let content = fs::read_to_string(dir.path().join("lynxx.toml")).unwrap();
        assert!(content.contains("[gateway]"));
        assert!(!content.contains("old"));
    }

    #[test]
    fn template_is_valid_toml() {
        let parsed: Result<toml::Value, _> = toml::from_str(TEMPLATE);
        assert!(parsed.is_ok(), "template is not valid TOML: {:?}", parsed.err());
    }
}
