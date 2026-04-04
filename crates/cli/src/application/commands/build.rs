use super::load_project;
use crate::application::{project::NexaProject, updater};
use crate::infrastructure::ui;
// nexa_compiler is an internal workspace crate; see its lib.rs for the semver-exempt notice.
use nexa_compiler::{compile_project_file, compile_to_bundle, decode_nxb, CodeGenerator};
use nexa_server::{build_router, AppState};
use notify::{Config as WatchConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

// ── Build ─────────────────────────────────────────────────────────────────────

pub fn build(project_dir: Option<PathBuf>) {
    updater::check_and_notify("stable");
    let proj = load_project(project_dir);
    let modules = proj
        .compiler
        .active_modules(&proj.project.modules)
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    let mut lock_entries: Vec<(String, Vec<BuildLockEntry>)> = Vec::new();
    for mod_name in &modules {
        let sp = ui::spinner(format!("Compiling module '{mod_name}'…"));
        match compile_project_file(
            &proj.module_entry(mod_name),
            &proj.module_src_root(mod_name),
            proj.root(),
            mod_name,
        ) {
            Ok(result) => {
                write_dist_inner(&proj.dist_dir(mod_name), result, mod_name);
                ui::done(
                    &sp,
                    format!("  {mod_name}  →  {}", proj.dist_dir(mod_name).display()),
                );
                let sources =
                    fingerprint_module_sources(&proj.module_src_root(mod_name), proj.root());
                lock_entries.push((mod_name.clone(), sources));
            }
            Err(e) => ui::fail(&sp, e.to_string()),
        }
    }

    let refs: Vec<(&str, Vec<BuildLockEntry>)> =
        lock_entries.iter().map(|(n, e)| (n.as_str(), e.clone())).collect();
    save_build_lock(proj.root(), &refs);
    ui::info(format!("Build OK — {} module(s) compiled", modules.len()));
}

// ── Run / Dev server ──────────────────────────────────────────────────────────

pub async fn run(
    bundle: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    port_override: Option<u16>,
    watch: bool,
) {
    if let Some(bundle_path) = bundle {
        if bundle_path
            .extension()
            .map(|e| e == "nexa")
            .unwrap_or(false)
        {
            run_from_bundle(bundle_path, port_override).await;
            return;
        } else {
            ui::die(format!(
                "expected a .nexa file, got '{}'",
                bundle_path.display()
            ));
        }
    }

    let proj = load_project(project_dir);
    let mod_name = proj.main_module_name().to_string();
    let sp = ui::spinner(format!("Compiling module '{mod_name}'…"));
    let result = match compile_project_file(
        &proj.main_entry(),
        &proj.main_src_root(),
        proj.root(),
        &mod_name,
    ) {
        Ok(r) => r,
        Err(e) => ui::fail(&sp, e.to_string()),
    };
    ui::done(&sp, "Compiled");

    let dist = proj.dist_dir(&mod_name);
    let _ = fs::create_dir_all(&dist);
    let _ = fs::write(dist.join("index.html"), &result.html);
    let _ = fs::write(dist.join("app.js"), &result.js);

    let port = port_override.unwrap_or(3000);
    let state = Arc::new(AppState::new(result.html, result.js, port));

    if watch {
        ui::info(format!("Watch mode  →  {}", proj.main_src_root().display()));
        let state_clone = state.clone();
        let proj_clone = proj.clone();
        tokio::spawn(async move {
            watch_task(state_clone, proj_clone).await;
        });
    }

    let router = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            ui::die(format!("Cannot bind to {addr}: {e}"));
        });

    ui::blank();
    println!("  \x1b[1;32m●\x1b[0m  Nexa dev server  →  \x1b[1mhttp://localhost:{port}\x1b[0m");
    if watch {
        ui::hint("     Hot reload enabled — watching for .nx changes");
    }
    ui::blank();

    axum::serve(listener, router.into_make_service())
        .await
        .unwrap();
}

// ── Package ───────────────────────────────────────────────────────────────────

pub fn package(
    project_dir: Option<PathBuf>,
    module_override: Option<String>,
    output: Option<PathBuf>,
) {
    let proj = load_project(project_dir);
    let mod_name = module_override.unwrap_or_else(|| proj.main_module_name().to_string());
    let app_name = proj.project.name.clone();
    let app_version = proj.project.version.clone();
    let bundle_name = format!("{app_name}-{mod_name}");

    let sp = ui::spinner(format!("Packaging {bundle_name} v{app_version}…"));

    let bundle = match compile_to_bundle(
        &proj.module_entry(&mod_name),
        &proj.module_src_root(&mod_name),
        proj.root(),
        &mod_name,
        &bundle_name,
        &app_version,
    ) {
        Ok(b) => b,
        Err(e) => ui::fail(&sp, e.to_string()),
    };

    let out_path = output.unwrap_or_else(|| PathBuf::from(format!("{bundle_name}.nexa")));
    let file = fs::File::create(&out_path)
        .unwrap_or_else(|e| ui::fail(&sp, format!("cannot create {}: {e}", out_path.display())));

    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("app.nxb", opts)
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP start app.nxb: {e}")));
    zip.write_all(&bundle.nxb)
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP write nxb: {e}")));
    zip.start_file("manifest.json", opts)
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP start manifest.json: {e}")));
    zip.write_all(bundle.manifest.as_bytes())
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP write manifest: {e}")));
    zip.start_file("signature.sig", opts)
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP start signature.sig: {e}")));
    zip.write_all(bundle.signature.as_bytes())
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP write signature: {e}")));
    let src_entry = format!("src/{}", bundle.source_filename);
    zip.start_file(&src_entry, opts)
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP start src entry: {e}")));
    zip.write_all(bundle.source.as_bytes())
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP write source: {e}")));
    zip.finish()
        .unwrap_or_else(|e| ui::fail(&sp, format!("ZIP finalize: {e}")));

    ui::done(&sp, format!("Package OK  →  {}", out_path.display()));
}

// ── write_dist (Q2 fixed: no panicking .expect) ───────────────────────────────

fn write_dist_inner(dist_dir: &Path, result: nexa_compiler::CompileResult, label: &str) {
    fs::create_dir_all(dist_dir).unwrap_or_else(|e| {
        ui::die(format!("cannot create dist dir for '{label}': {e}"));
    });
    fs::write(dist_dir.join("index.html"), &result.html).unwrap_or_else(|e| {
        ui::die(format!("cannot write index.html for '{label}': {e}"));
    });
    fs::write(dist_dir.join("app.js"), &result.js).unwrap_or_else(|e| {
        ui::die(format!("cannot write app.js for '{label}': {e}"));
    });
}

// ── Bundle runner ─────────────────────────────────────────────────────────────

async fn run_from_bundle(bundle_path: PathBuf, port_override: Option<u16>) {
    let sp = ui::spinner(format!("Loading bundle {}…", bundle_path.display()));

    let file = fs::File::open(&bundle_path)
        .unwrap_or_else(|e| ui::fail(&sp, format!("cannot open {}: {e}", bundle_path.display())));
    let mut archive = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| ui::fail(&sp, format!("invalid .nexa file: {e}")));

    let nxb_bytes = read_zip_entry(&mut archive, "app.nxb", &sp);
    let manifest_bytes = read_zip_entry(&mut archive, "manifest.json", &sp);
    let sig_bytes = read_zip_entry(&mut archive, "signature.sig", &sp);

    let expected_sig = String::from_utf8_lossy(&sig_bytes).trim().to_string();
    let mut hasher = Sha256::new();
    hasher.update(&nxb_bytes);
    hasher.update(&manifest_bytes);
    let actual_sig = format!("{:x}", hasher.finalize());

    if actual_sig != expected_sig {
        ui::fail(
            &sp,
            "bundle signature validation failed — file may be corrupted or tampered",
        );
    }

    let program = decode_nxb(&nxb_bytes)
        .unwrap_or_else(|e| ui::fail(&sp, format!("failed to decode bundle: {e}")));

    let result = CodeGenerator::new()
        .generate(&program)
        .unwrap_or_else(|e| ui::fail(&sp, format!("codegen failed: {e}")));

    ui::done(&sp, "Bundle loaded  ✓  signature OK");

    let port = port_override
        .or_else(|| program.server.as_ref().map(|s| s.port))
        .unwrap_or(3000);

    let state = Arc::new(AppState::new(result.html, result.js, port));
    let router = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            ui::die(format!("Cannot bind to {addr}: {e}"));
        });

    ui::blank();
    println!(
        "  \x1b[1;32m●\x1b[0m  Nexa dev server (bundle)  →  \x1b[1mhttp://localhost:{port}\x1b[0m"
    );
    ui::blank();

    axum::serve(listener, router.into_make_service())
        .await
        .unwrap();
}

/// Read a named entry from a ZIP archive, failing with a user-friendly message on error.
fn read_zip_entry(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
    sp: &indicatif::ProgressBar,
) -> Vec<u8> {
    let mut entry = archive.by_name(name).unwrap_or_else(|_| {
        ui::fail(sp, format!(".nexa bundle is missing '{name}'"));
    });
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).unwrap_or_else(|e| {
        ui::fail(sp, format!("failed to read '{name}' from bundle: {e}"));
    });
    buf
}

// ── File watcher (hot reload) ─────────────────────────────────────────────────

async fn watch_task(state: Arc<AppState>, proj: NexaProject) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(32);

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, _>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        WatchConfig::default(),
    )
    .unwrap_or_else(|e| ui::die(format!("Watch error: {e}")));

    watcher
        .watch(&proj.modules_dir(), RecursiveMode::Recursive)
        .unwrap_or_else(|e| ui::die(format!("Watch error: {e}")));

    while let Some(event) = rx.recv().await {
        let has_nx = event
            .paths
            .iter()
            .any(|p| p.extension().map(|e| e == "nx").unwrap_or(false));

        if !has_nx {
            continue;
        }

        let mod_name = proj.main_module_name().to_string();
        let sp = ui::spinner("Recompiling…");
        match compile_project_file(
            &proj.main_entry(),
            &proj.main_src_root(),
            proj.root(),
            &mod_name,
        ) {
            Ok(result) => {
                state.update(result.html, result.js).await;
                ui::done(&sp, "Recompiled  →  reload sent");
            }
            Err(e) => {
                sp.finish_and_clear();
                ui::error(e.to_string());
            }
        }
    }
}

// ── Build lockfile ────────────────────────────────────────────────────────────

/// One source-file entry in `nexa-build.lock`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub(super) struct BuildLockEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct BuildLockfile {
    nexa_version: String,
    modules: std::collections::HashMap<String, Vec<BuildLockEntry>>,
}

pub(super) fn fingerprint_module_sources(
    src_root: &Path,
    project_root: &Path,
) -> Vec<BuildLockEntry> {
    use sha2::{Digest, Sha256};
    let mut entries = Vec::new();
    let Ok(walk) = fs::read_dir(src_root) else {
        return entries;
    };
    let mut queue: Vec<_> = walk.flatten().collect();
    while let Some(entry) = queue.pop() {
        let p = entry.path();
        if p.is_dir() {
            if let Ok(sub) = fs::read_dir(&p) {
                queue.extend(sub.flatten());
            }
        } else if p.extension().map(|e| e == "nx").unwrap_or(false) {
            if let Ok(bytes) = fs::read(&p) {
                let mut h = Sha256::new();
                h.update(&bytes);
                let sha256 = hex::encode(h.finalize());
                let rel = p
                    .strip_prefix(project_root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                entries.push(BuildLockEntry { path: rel, sha256 });
            }
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

pub(super) fn save_build_lock(
    project_root: &Path,
    module_entries: &[(&str, Vec<BuildLockEntry>)],
) {
    let lock_path = project_root.join("nexa-build.lock");
    let mut lock: BuildLockfile = fs::read_to_string(&lock_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    lock.nexa_version = env!("CARGO_PKG_VERSION").to_string();
    for (mod_name, entries) in module_entries {
        lock.modules.insert(mod_name.to_string(), entries.clone());
    }
    match serde_json::to_string_pretty::<BuildLockfile>(&lock) {
        Ok(json) => {
            if let Err(e) = fs::write(&lock_path, json) {
                ui::warn(format!("could not write nexa-build.lock: {e}"));
            }
        }
        Err(e) => ui::warn(format!("could not serialize nexa-build.lock: {e}")),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    fn sha256_hex(content: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(content);
        hex::encode(h.finalize())
    }

    // ── fingerprint_module_sources ────────────────────────────────────────────

    #[test]
    fn fingerprint_empty_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let entries = fingerprint_module_sources(tmp.path(), tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn fingerprint_ignores_non_nx_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("README.md"), "hello").unwrap();
        fs::write(tmp.path().join("config.json"), "{}").unwrap();
        fs::write(tmp.path().join("script.js"), "x=1").unwrap();
        let entries = fingerprint_module_sources(tmp.path(), tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn fingerprint_hashes_nx_files_correctly() {
        let tmp = TempDir::new().unwrap();
        let content = b"app MyApp {}";
        fs::write(tmp.path().join("app.nx"), content).unwrap();

        let entries = fingerprint_module_sources(tmp.path(), tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha256, sha256_hex(content));
    }

    #[test]
    fn fingerprint_results_are_sorted_by_path() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("z_last.nx"), "z").unwrap();
        fs::write(tmp.path().join("a_first.nx"), "a").unwrap();
        fs::write(tmp.path().join("m_middle.nx"), "m").unwrap();

        let entries = fingerprint_module_sources(tmp.path(), tmp.path());
        assert_eq!(entries.len(), 3);
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn fingerprint_recurses_into_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("views");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("app.nx"), "root").unwrap();
        fs::write(sub.join("home.nx"), "home").unwrap();

        let entries = fingerprint_module_sources(tmp.path(), tmp.path());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn fingerprint_uses_relative_paths_from_project_root() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("modules").join("core").join("src").join("main");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("app.nx"), "x").unwrap();

        let entries = fingerprint_module_sources(&src, tmp.path());
        assert_eq!(entries.len(), 1);
        // Path should be relative, not absolute
        assert!(!entries[0].path.starts_with('/'));
        assert!(entries[0].path.contains("app.nx"));
    }

    // ── save_build_lock ───────────────────────────────────────────────────────

    #[test]
    fn save_build_lock_creates_lock_file() {
        let tmp = TempDir::new().unwrap();
        let entry = BuildLockEntry {
            path: "modules/core/src/main/app.nx".to_string(),
            sha256: "abc123".to_string(),
        };
        save_build_lock(tmp.path(), &[("core", vec![entry])]);

        assert!(tmp.path().join("nexa-build.lock").exists());
    }

    #[test]
    fn save_build_lock_file_is_valid_json() {
        let tmp = TempDir::new().unwrap();
        save_build_lock(tmp.path(), &[("core", vec![])]);

        let raw = fs::read_to_string(tmp.path().join("nexa-build.lock")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(val.get("nexa_version").is_some());
        assert!(val.get("modules").is_some());
    }

    #[test]
    fn save_build_lock_stores_module_entries() {
        let tmp = TempDir::new().unwrap();
        let entry = BuildLockEntry {
            path: "modules/core/src/main/app.nx".to_string(),
            sha256: "deadbeef".to_string(),
        };
        save_build_lock(tmp.path(), &[("core", vec![entry.clone()])]);

        let raw = fs::read_to_string(tmp.path().join("nexa-build.lock")).unwrap();
        let lock: BuildLockfile = serde_json::from_str(&raw).unwrap();
        let core_entries = lock.modules.get("core").unwrap();
        assert_eq!(core_entries.len(), 1);
        assert_eq!(core_entries[0], entry);
    }

    #[test]
    fn save_build_lock_merges_with_existing_modules() {
        let tmp = TempDir::new().unwrap();

        // First call: save "core"
        save_build_lock(
            tmp.path(),
            &[("core", vec![BuildLockEntry {
                path: "core/app.nx".to_string(),
                sha256: "hash1".to_string(),
            }])],
        );

        // Second call: save "api" — should not clobber "core"
        save_build_lock(
            tmp.path(),
            &[("api", vec![BuildLockEntry {
                path: "api/app.nx".to_string(),
                sha256: "hash2".to_string(),
            }])],
        );

        let raw = fs::read_to_string(tmp.path().join("nexa-build.lock")).unwrap();
        let lock: BuildLockfile = serde_json::from_str(&raw).unwrap();
        assert!(lock.modules.contains_key("core"), "core entries should be preserved");
        assert!(lock.modules.contains_key("api"), "api entries should be added");
    }
}
