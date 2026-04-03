//! Self-updater for the Nexa CLI.
//!
//! Architecture:
//!  - On every `nexa build / run / install`, we read a local cache file
//!    (~/.nexa/.update_check.json). If a newer version is recorded there, we
//!    print a one-line notice AFTER the command output. Then, if the cache is
//!    older than 24 hours, we spawn a background thread to refresh it —
//!    completely transparent to the user.
//!
//!  - `nexa update [--channel <c>]` downloads the prebuilt binary for the
//!    current platform, verifies its SHA-256 checksum, and atomically replaces
//!    the running executable.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── compile-time version (set by build.rs, overridable via NEXA_RELEASE_VERSION)
pub const CURRENT_VERSION: &str = env!("NEXA_BUILD_VERSION");
const REPO: &str = "nexa-lang-org/nexa-lang";
const GITHUB_API: &str = "https://api.github.com";
// Refresh the cached update check at most once every 24 hours
const CACHE_TTL_SECS: u64 = 86_400;

// ── platform asset name ───────────────────────────────────────────────────────
//
// Must match the asset names produced by the release/snapshot workflows.
//
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const PLATFORM_ASSET: Option<&str> = Some("nexa-linux-x86_64.tar.gz");

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const PLATFORM_ASSET: Option<&str> = Some("nexa-linux-aarch64.tar.gz");

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const PLATFORM_ASSET: Option<&str> = Some("nexa-macos-x86_64.tar.gz");

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PLATFORM_ASSET: Option<&str> = Some("nexa-macos-aarch64.tar.gz");

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const PLATFORM_ASSET: Option<&str> = Some("nexa-windows-x86_64.zip");

// Fallback — unsupported platform
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const PLATFORM_ASSET: Option<&str> = None;

// ── types ─────────────────────────────────────────────────────────────────────

/// Minimal subset of the GitHub Releases API response we care about.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    /// Release title, e.g. "Snapshot – 2026-04-03 10:00 UTC (abc1234)"
    #[serde(default)]
    name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Cached result of the last update check.
#[derive(Debug, Serialize, Deserialize, Default)]
struct UpdateCache {
    /// Unix timestamp of the last successful check.
    checked_at: u64,
    /// Channel that was checked ("stable" or "snapshot").
    channel: String,
    /// Latest version found (e.g. "0.2.0"), empty string if check failed.
    latest_version: String,
    /// Download URL for the current platform (may be empty).
    download_url: String,
    /// Checksum download URL (may be empty).
    checksum_url: String,
}

/// Information about an available update, ready to apply.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub checksum_url: String,
}

// ── semver helpers ────────────────────────────────────────────────────────────

/// Parse "v0.1.2" or "0.1.2" → (0, 1, 2).
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim_start_matches('v');
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.split('-').next()?.parse().ok()?; // strip pre-release suffix
    Some((major, minor, patch))
}

/// Returns `true` if `candidate` is strictly newer than `current` (semver).
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

/// Extract the short git SHA from a version string.
/// "0.1.0-snapshot.abc1234" → Some("abc1234")
/// "0.1.0-dev.abc1234"      → Some("abc1234")
/// "0.1.0"                  → None  (stable install, no SHA)
fn version_sha(v: &str) -> Option<&str> {
    let suffix = v.split_once('-')?.1; // "snapshot.abc1234"
    suffix.split_once('.').map(|(_, sha)| sha)
}

/// Extract the short git SHA from a snapshot release title.
/// "Snapshot – 2026-04-03 10:00 UTC (abc1234)" → Some("abc1234")
fn title_sha(title: &str) -> Option<&str> {
    let start = title.rfind('(')? + 1;
    let end = title.rfind(')')?;
    if start < end {
        Some(title[start..end].trim())
    } else {
        None
    }
}

/// For snapshot channel: decide whether we should update.
/// - If current binary has no SHA (was installed from stable), always update.
/// - If SHAs differ, update.
/// - If SHAs match, already up to date.
fn snapshot_needs_update(release: &GithubRelease) -> bool {
    match version_sha(CURRENT_VERSION) {
        None => true, // stable binary → offer snapshot upgrade
        Some(local_sha) => match title_sha(&release.name) {
            None => true, // can't determine → offer update
            Some(remote_sha) => local_sha != remote_sha,
        },
    }
}

// ── cache helpers ─────────────────────────────────────────────────────────────

fn nexa_home() -> PathBuf {
    dirs_home()
        .map(|h| h.join(".nexa"))
        .unwrap_or_else(|| PathBuf::from(".nexa"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn cache_path() -> PathBuf {
    nexa_home().join(".update_check.json")
}

fn read_cache() -> UpdateCache {
    let path = cache_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_cache(cache: &UpdateCache) {
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let path = cache_path();
        let _ = fs::create_dir_all(path.parent().unwrap());
        let _ = fs::write(path, json);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_is_stale(cache: &UpdateCache, channel: &str) -> bool {
    cache.channel != channel
        || cache.checked_at == 0
        || now_secs().saturating_sub(cache.checked_at) > CACHE_TTL_SECS
}

// ── GitHub API fetch ──────────────────────────────────────────────────────────

fn fetch_release(channel: &str) -> Result<GithubRelease, String> {
    let url = match channel {
        "snapshot" => format!("{GITHUB_API}/repos/{REPO}/releases/tags/snapshot"),
        _ => format!("{GITHUB_API}/repos/{REPO}/releases/latest"),
    };

    let user_agent = format!("nexa-cli/{CURRENT_VERSION}");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(&user_agent)
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(&url).send().map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<GithubRelease>().map_err(|e| e.to_string())
}

/// Fetch the latest release and return an `UpdateInfo` if a newer version is
/// available for the current platform. Returns `None` on network errors or
/// if already up-to-date.
pub fn check_for_update(channel: &str) -> Option<UpdateInfo> {
    let asset_name = PLATFORM_ASSET?;
    let release = fetch_release(channel).ok()?;

    // Snapshot channel: compare git SHAs, not semver tags.
    // The tag is always "snapshot" which is not a valid semver.
    let needs_update = if channel == "snapshot" {
        snapshot_needs_update(&release)
    } else {
        is_newer(&release.tag_name, CURRENT_VERSION)
    };

    if !needs_update {
        return None;
    }

    let asset = release.assets.iter().find(|a| a.name == asset_name)?;
    let checksum_name = format!("{}.sha256", asset_name);
    let checksum_asset = release.assets.iter().find(|a| a.name == checksum_name)?;

    // For snapshot, derive a human-readable version from the release title SHA.
    let version = if channel == "snapshot" {
        let base = parse_version(CURRENT_VERSION)
            .map(|(ma, mi, pa)| format!("{ma}.{mi}.{pa}"))
            .unwrap_or_else(|| "0.1.0".to_string());
        let sha = title_sha(&release.name).unwrap_or("latest");
        format!("{base}-snapshot.{sha}")
    } else {
        release.tag_name.trim_start_matches('v').to_string()
    };

    Some(UpdateInfo {
        version,
        download_url: asset.browser_download_url.clone(),
        checksum_url: checksum_asset.browser_download_url.clone(),
    })
}

// ── background check + passive notification ───────────────────────────────────

/// Call this at the start of user-visible commands (build, run, install, …).
///
/// 1. Reads the local cache — prints a one-line notice if a newer version is
///    recorded there.
/// 2. If the cache is stale (> 24 h), spawns a background thread to refresh
///    it silently. The updated result will appear on the *next* command run.
pub fn check_and_notify(channel: &str) {
    let cache = read_cache();

    // Show notice if the cached latest is newer than what's installed.
    // For snapshot: compare SHAs (versions like "0.1.0-snapshot.abc1234").
    let cached_is_newer = if channel == "snapshot" {
        !cache.latest_version.is_empty()
            && cache.channel == channel
            && version_sha(&cache.latest_version) != version_sha(CURRENT_VERSION)
    } else {
        !cache.latest_version.is_empty()
            && cache.channel == channel
            && is_newer(&cache.latest_version, CURRENT_VERSION)
    };
    if cached_is_newer {
        eprintln!(
            "\n  \x1b[1;33m⬆  Nexa {} is available\x1b[0m (you have {}). \
             Run \x1b[1mnexa update\x1b[0m to upgrade.\n",
            cache.latest_version, CURRENT_VERSION
        );
    }

    // Refresh cache in background if stale
    if cache_is_stale(&cache, channel) {
        let channel = channel.to_string();
        std::thread::spawn(move || {
            if let Some(asset_name) = PLATFORM_ASSET {
                if let Ok(release) = fetch_release(&channel) {
                    let new_cache = if let Some(asset) =
                        release.assets.iter().find(|a| a.name == asset_name)
                    {
                        let checksum_name = format!("{}.sha256", asset_name);
                        let checksum_url = release
                            .assets
                            .iter()
                            .find(|a| a.name == checksum_name)
                            .map(|a| a.browser_download_url.clone())
                            .unwrap_or_default();
                        UpdateCache {
                            checked_at: now_secs(),
                            channel: channel.clone(),
                            latest_version: release.tag_name.trim_start_matches('v').to_string(),
                            download_url: asset.browser_download_url.clone(),
                            checksum_url,
                        }
                    } else {
                        // Release exists but no binary for this platform
                        UpdateCache {
                            checked_at: now_secs(),
                            channel,
                            ..Default::default()
                        }
                    };
                    write_cache(&new_cache);
                }
            }
        });
    }
}

// ── self-update ───────────────────────────────────────────────────────────────

/// Download `url` into a temp file inside `tmp_dir` and return its path.
fn download_to_temp(url: &str, filename: &str) -> Result<PathBuf, String> {
    let tmp_dir = std::env::temp_dir().join("nexa-update");
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let dest = tmp_dir.join(filename);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(format!("nexa-cli/{CURRENT_VERSION}"))
        .build()
        .map_err(|e| e.to_string())?;

    let mut resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} while downloading {url}", resp.status()));
    }

    let mut file = fs::File::create(&dest).map_err(|e| e.to_string())?;
    resp.copy_to(&mut file).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Verify the archive against its `.sha256` sidecar file.
fn verify_sha256(archive: &PathBuf, checksum_file: &PathBuf) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let mut f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let computed = format!("{:x}", hasher.finalize());

    let content = fs::read_to_string(checksum_file).map_err(|e| e.to_string())?;
    let expected = content
        .split_whitespace()
        .next()
        .ok_or("empty checksum file")?;

    if computed != expected {
        return Err(format!(
            "checksum mismatch\n  expected: {expected}\n  computed: {computed}"
        ));
    }
    Ok(())
}

/// Extract `nexa` (or `nexa.exe`) from the archive into `dest_dir`.
fn extract_binary(archive: &PathBuf, dest_dir: &Path) -> Result<PathBuf, String> {
    let archive_str = archive.to_string_lossy();

    if archive_str.ends_with(".tar.gz") {
        // Unix — shell out to system tar (universally available on macOS/Linux)
        let status = std::process::Command::new("tar")
            .args(["-xzf", &archive_str, "-C", &dest_dir.to_string_lossy()])
            .status()
            .map_err(|e| format!("tar: {e}"))?;
        if !status.success() {
            return Err(format!("tar exited with {status}"));
        }
        Ok(dest_dir.join("nexa"))
    } else if archive_str.ends_with(".zip") {
        // Windows — use the zip crate
        let file = fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut entry = zip.by_name("nexa.exe").map_err(|e| e.to_string())?;
        let out_path = dest_dir.join("nexa.exe");
        let mut out = fs::File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        Ok(out_path)
    } else {
        Err(format!("unknown archive format: {archive_str}"))
    }
}

/// Atomically replace the running binary with `new_binary`.
fn replace_binary(new_binary: &PathBuf) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("cannot locate current executable: {e}"))?;

    // Resolve symlinks so we replace the real file
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(new_binary, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        // rename() is atomic on Unix (same filesystem)
        let tmp = current_exe.with_extension("nexa-update-tmp");
        fs::copy(new_binary, &tmp).map_err(|e| format!("copy: {e}"))?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &current_exe).map_err(|e| format!("rename: {e}"))?;
    }

    #[cfg(windows)]
    {
        // Windows: can't overwrite a running exe — rename it away first
        let old = current_exe.with_extension("old");
        fs::rename(&current_exe, &old).map_err(|e| format!("rename old: {e}"))?;
        if let Err(e) = fs::rename(new_binary, &current_exe) {
            // Try to restore
            let _ = fs::rename(&old, &current_exe);
            return Err(format!("rename new: {e}"));
        }
        let _ = fs::remove_file(&old); // best-effort
    }

    Ok(())
}

/// Full self-update: download, verify, replace. Called by `nexa update`.
pub fn perform_update(info: &UpdateInfo) -> Result<(), String> {
    let asset_name = PLATFORM_ASSET.ok_or("no prebuilt binary for this platform")?;
    let checksum_name = format!("{}.sha256", asset_name);

    println!("  Downloading nexa {}…", info.version);
    let archive = download_to_temp(&info.download_url, asset_name)?;

    println!("  Verifying checksum…");
    let checksum_file = download_to_temp(&info.checksum_url, &checksum_name)?;
    verify_sha256(&archive, &checksum_file)?;
    println!("  \x1b[1;32m✓\x1b[0m Checksum OK");

    println!("  Extracting binary…");
    let tmp_dir = std::env::temp_dir().join("nexa-update");
    let new_bin = extract_binary(&archive, &tmp_dir)?;

    println!("  Installing…");
    replace_binary(&new_bin)?;

    // Clean up temp files
    let _ = fs::remove_file(&archive);
    let _ = fs::remove_file(&checksum_file);
    let _ = fs::remove_file(&new_bin);

    // Invalidate the update cache so we don't nag again immediately
    write_cache(&UpdateCache {
        checked_at: now_secs(),
        channel: "stable".into(),
        latest_version: info.version.clone(),
        ..Default::default()
    });

    Ok(())
}

// ── `nexa update` command entry point ────────────────────────────────────────

pub fn run_update_command(channel_override: Option<String>) {
    let asset_name = match PLATFORM_ASSET {
        Some(a) => a,
        None => {
            eprintln!(
                "error: no prebuilt binary available for this platform.\n\
                 Please build from source: https://github.com/{REPO}#installation"
            );
            std::process::exit(1);
        }
    };

    let channel = channel_override.as_deref().unwrap_or("stable");

    println!("\n  Checking for updates (channel: {channel})…");

    let info = match check_for_update(channel) {
        Some(i) => i,
        None => {
            println!(
                "  \x1b[1;32m✓\x1b[0m Nexa {} is already up to date.",
                CURRENT_VERSION
            );
            return;
        }
    };

    println!(
        "  \x1b[1;33m⬆\x1b[0m  Update available: {} → {}",
        CURRENT_VERSION, info.version
    );
    println!("  Platform asset  : {asset_name}");
    println!("  Download URL    : {}", info.download_url);
    println!();

    match perform_update(&info) {
        Ok(()) => {
            println!();
            println!(
                "  \x1b[1;32m✓\x1b[0m  Nexa updated to version \x1b[1m{}\x1b[0m",
                info.version
            );
            println!();
        }
        Err(e) => {
            eprintln!("error: update failed: {e}");
            eprintln!(
                "       You can install manually: \
                 https://github.com/{REPO}/releases/tag/v{}",
                info.version
            );
            std::process::exit(1);
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("v2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("invalid"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
    }

    #[test]
    fn cache_staleness() {
        let fresh = UpdateCache {
            checked_at: now_secs(),
            channel: "stable".into(),
            ..Default::default()
        };
        assert!(!cache_is_stale(&fresh, "stable"));

        let old = UpdateCache {
            checked_at: now_secs() - CACHE_TTL_SECS - 1,
            channel: "stable".into(),
            ..Default::default()
        };
        assert!(cache_is_stale(&old, "stable"));

        // Wrong channel → stale
        assert!(cache_is_stale(&fresh, "snapshot"));
    }
}
