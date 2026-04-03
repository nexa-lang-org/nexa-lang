use std::process::Command;

fn main() {
    // Re-run this script if git HEAD changes (new commit) or if the
    // override env var is set explicitly.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-env-changed=NEXA_RELEASE_VERSION");

    // Allow CI to inject a version string (e.g. "0.2.0" for stable releases,
    // "0.1.0-snapshot.abc1234" for snapshot builds).
    // Falls back to CARGO_PKG_VERSION + git SHA when not set.
    let version = if let Ok(v) = std::env::var("NEXA_RELEASE_VERSION") {
        v
    } else {
        let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let git_sha = short_sha().unwrap_or_default();
        if git_sha.is_empty() {
            pkg_version
        } else {
            format!("{pkg_version}-dev.{git_sha}")
        }
    };

    println!("cargo:rustc-env=NEXA_BUILD_VERSION={version}");
}

fn short_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
