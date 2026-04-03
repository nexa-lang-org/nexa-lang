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

/// Load saved credentials from `~/.nexa/credentials.json`.
/// Returns `None` if the file does not exist or cannot be parsed.
pub fn load() -> Option<Credentials> {
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
