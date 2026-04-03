use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Registry credentials stored in `~/.nexa/credentials.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Registry URL this token was issued for.
    pub registry: String,
    /// JWT bearer token returned by `POST /auth/login` or `/auth/register`.
    pub token: String,
}

fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".nexa").join("credentials.json")
}

/// Load credentials, with the following priority:
///
/// 1. `NEXA_TOKEN` environment variable (used in CI/CD pipelines).
///    `NEXA_REGISTRY` can optionally override the registry URL.
/// 2. `~/.nexa/credentials.json` (set by `nexa login`).
///
/// Returns `None` if neither source provides a token.
pub fn load() -> Option<Credentials> {
    // CI/CD path: prefer env var so pipelines don't need interactive login
    if let Ok(token) = std::env::var("NEXA_TOKEN") {
        if !token.is_empty() {
            let registry = std::env::var("NEXA_REGISTRY")
                .unwrap_or_else(|_| "https://registry.nexa-lang.org".to_string());
            return Some(Credentials { registry, token });
        }
    }
    let path = credentials_path();
    let text = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist a registry token to `~/.nexa/credentials.json`.
/// Creates the `~/.nexa/` directory if it does not yet exist.
pub fn save(registry: &str, token: &str) {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let creds = Credentials {
        registry: registry.to_string(),
        token: token.to_string(),
    };
    let json = serde_json::to_string_pretty(&creds).expect("serialize credentials");
    fs::write(&path, json).unwrap_or_else(|e| {
        eprintln!(
            "warning: could not save credentials to {}: {e}",
            path.display()
        );
    });
}
