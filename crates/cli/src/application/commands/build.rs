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

    // Incremental build: load existing lock once upfront so we can skip
    // modules whose sources haven't changed and whose dist/ output still exists.
    let existing_lock = load_build_lock(proj.root());

    let mut lock_entries: Vec<(String, Vec<BuildLockEntry>)> = Vec::new();
    let mut compiled = 0usize;
    let mut skipped = 0usize;

    let pb = ui::progress_bar("Compiling…", modules.len() as u64);

    for mod_name in &modules {
        let current_sources =
            fingerprint_module_sources(&proj.module_src_root(mod_name), proj.root());

        if is_module_up_to_date(&existing_lock, mod_name, &current_sources, &proj.dist_dir(mod_name)) {
            lock_entries.push((mod_name.clone(), current_sources));
            skipped += 1;
            pb.inc(1);
            continue;
        }

        match compile_project_file(
            &proj.module_entry(mod_name),
            &proj.module_src_root(mod_name),
            proj.root(),
            mod_name,
        ) {
            Ok(result) => {
                write_dist_inner(&proj.dist_dir(mod_name), result, mod_name);
                lock_entries.push((mod_name.clone(), current_sources));
                compiled += 1;
                pb.inc(1);
            }
            Err(e) => ui::bar_fail(&pb, e.to_string()),
        }
    }

    let refs: Vec<(&str, Vec<BuildLockEntry>)> =
        lock_entries.iter().map(|(n, e)| (n.as_str(), e.clone())).collect();
    save_build_lock(proj.root(), &refs);

    let summary = match (compiled, skipped) {
        (c, 0) => format!("Build OK — {c} module(s) compiled"),
        (0, s) => format!("Build OK — {s} module(s) up to date (nothing to compile)"),
        (c, s) => format!("Build OK — {c} compiled, {s} up to date"),
    };
    ui::bar_done(&pb, summary);
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
    let pb = ui::progress_bar("Compiling…", 2);
    let result = match compile_project_file(
        &proj.main_entry(),
        &proj.main_src_root(),
        proj.root(),
        &mod_name,
    ) {
        Ok(r) => r,
        Err(e) => ui::bar_fail(&pb, e.to_string()),
    };
    pb.inc(1);

    let dist = proj.dist_dir(&mod_name);
    let _ = fs::create_dir_all(&dist);
    let _ = fs::write(dist.join("index.html"), &result.html);
    let _ = fs::write(dist.join("app.js"), &result.js);
    pb.inc(1);
    ui::bar_done(&pb, "Compiled");

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

    let pb = ui::progress_bar("Packaging…", 2);

    let bundle = match compile_to_bundle(
        &proj.module_entry(&mod_name),
        &proj.module_src_root(&mod_name),
        proj.root(),
        &mod_name,
        &bundle_name,
        &app_version,
    ) {
        Ok(b) => b,
        Err(e) => ui::bar_fail(&pb, e.to_string()),
    };
    pb.inc(1);

    let out_path = output.unwrap_or_else(|| PathBuf::from(format!("{bundle_name}.nexa")));
    let file = fs::File::create(&out_path)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("cannot create {}: {e}", out_path.display())));

    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("app.nxb", opts)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP start app.nxb: {e}")));
    zip.write_all(&bundle.nxb)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP write nxb: {e}")));
    zip.start_file("manifest.json", opts)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP start manifest.json: {e}")));
    zip.write_all(bundle.manifest.as_bytes())
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP write manifest: {e}")));
    zip.start_file("signature.sig", opts)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP start signature.sig: {e}")));
    zip.write_all(bundle.signature.as_bytes())
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP write signature: {e}")));
    let src_entry = format!("src/{}", bundle.source_filename);
    zip.start_file(&src_entry, opts)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP start src entry: {e}")));
    zip.write_all(bundle.source.as_bytes())
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP write source: {e}")));
    zip.finish()
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("ZIP finalize: {e}")));
    pb.inc(1);

    ui::bar_done(&pb, format!("Package OK  →  {}", out_path.display()));
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
    let pb = ui::progress_bar("Loading bundle…", 4);

    let file = fs::File::open(&bundle_path)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("cannot open {}: {e}", bundle_path.display())));
    let mut archive = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("invalid .nexa file: {e}")));

    let nxb_bytes = read_zip_entry(&mut archive, "app.nxb", &pb);
    let manifest_bytes = read_zip_entry(&mut archive, "manifest.json", &pb);
    let sig_bytes = read_zip_entry(&mut archive, "signature.sig", &pb);
    pb.inc(1); // step 1: unpacked

    let expected_sig = String::from_utf8_lossy(&sig_bytes).trim().to_string();
    let mut hasher = Sha256::new();
    hasher.update(&nxb_bytes);
    hasher.update(&manifest_bytes);
    let actual_sig = format!("{:x}", hasher.finalize());

    if actual_sig != expected_sig {
        ui::bar_fail(
            &pb,
            "bundle signature validation failed — file may be corrupted or tampered",
        );
    }
    pb.inc(1); // step 2: signature OK

    let program = decode_nxb(&nxb_bytes)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("failed to decode bundle: {e}")));
    pb.inc(1); // step 3: decoded

    let result = CodeGenerator::new()
        .generate(&program)
        .unwrap_or_else(|e| ui::bar_fail(&pb, format!("codegen failed: {e}")));
    pb.inc(1); // step 4: codegen

    ui::bar_done(&pb, "Bundle loaded  ✓  signature OK");

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
    pb: &indicatif::ProgressBar,
) -> Vec<u8> {
    let mut entry = archive.by_name(name).unwrap_or_else(|_| {
        ui::bar_fail(pb, format!(".nexa bundle is missing '{name}'"));
    });
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).unwrap_or_else(|e| {
        ui::bar_fail(pb, format!("failed to read '{name}' from bundle: {e}"));
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
pub(super) struct BuildLockfile {
    nexa_version: String,
    modules: std::collections::HashMap<String, Vec<BuildLockEntry>>,
}

/// Load the `nexa-build.lock` file from the project root.
/// Returns a default (empty) lockfile if it does not exist or cannot be parsed.
pub(super) fn load_build_lock(project_root: &Path) -> BuildLockfile {
    fs::read_to_string(project_root.join("nexa-build.lock"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Return `true` if `mod_name` is up to date and does not need to be recompiled.
///
/// A module is considered up to date when ALL of the following hold:
///   1. The lock contains an entry for this module.
///   2. The current source fingerprints are identical to the locked fingerprints.
///   3. The compiled output (`app.js`) exists in `dist_dir` — prevents skipping
///      when someone manually deletes the dist output.
pub(super) fn is_module_up_to_date(
    lock: &BuildLockfile,
    mod_name: &str,
    current: &[BuildLockEntry],
    dist_dir: &Path,
) -> bool {
    if !dist_dir.join("app.js").exists() {
        return false;
    }
    match lock.modules.get(mod_name) {
        None => false,
        Some(prev) => prev == current,
    }
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

    // ── watch_task E2E helpers ────────────────────────────────────────────────

    /// Minimal project that compiles successfully.
    fn make_watch_project(dir: &std::path::Path) {
        fs::write(
            dir.join("project.json"),
            r#"{"name":"watch-test","version":"0.1.0","author":"Test","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("nexa-compiler.yaml"),
            "version: \"0.1\"\nmain_module: \"core\"\n",
        )
        .unwrap();
        let src_main = dir
            .join("modules")
            .join("core")
            .join("src")
            .join("main");
        fs::create_dir_all(&src_main).unwrap();
        fs::write(
            dir.join("modules").join("core").join("module.json"),
            r#"{"name":"core","main":"app.nx"}"#,
        )
        .unwrap();
        fs::write(
            src_main.join("app.nx"),
            "app App {\n  server { port: 3000; }\n  public window HomePage {\n    public render() => Component {\n      return Page { Text(\"Hi\") };\n    }\n  }\n  route \"/\" => HomePage;\n}\n",
        )
        .unwrap();
    }

    fn app_nx_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("modules")
            .join("core")
            .join("src")
            .join("main")
            .join("app.nx")
    }

    // ── watch_task: .nx change triggers recompile and reload signal ───────────

    /// Spawn watch_task, modify a .nx file, and wait for the "reload" broadcast.
    /// Covers the happy path of `nexa run --watch`.
    #[tokio::test]
    async fn watch_task_recompiles_on_nx_file_change() {
        use tokio::time::{sleep, timeout, Duration};

        let tmp = TempDir::new().unwrap();
        make_watch_project(tmp.path());

        let proj = crate::application::project::NexaProject::load(tmp.path()).unwrap();
        let state = Arc::new(nexa_server::AppState::new(
            String::new(),
            String::new(),
            0,
        ));
        let mut rx = state.tx.subscribe();

        let s = state.clone();
        let p = proj.clone();
        tokio::spawn(async move { watch_task(s, p).await });

        // Give the file-system watcher time to arm itself.
        sleep(Duration::from_millis(300)).await;

        // Touch the source file — watcher should pick this up.
        fs::write(
            app_nx_path(tmp.path()),
            "app App {\n  server { port: 3000; }\n  public window HomePage {\n    public render() => Component {\n      return Page { Text(\"Updated\") };\n    }\n  }\n  route \"/\" => HomePage;\n}\n",
        )
        .unwrap();

        // Expect a "reload" broadcast within 4 seconds.
        let msg = timeout(Duration::from_secs(4), rx.recv())
            .await
            .expect("expected a reload signal within 4 s after .nx file change")
            .expect("broadcast channel closed unexpectedly");
        assert_eq!(msg, "reload");
    }

    // ── watch_task: non-.nx changes are silently ignored ─────────────────────

    /// Writing a Markdown file must NOT trigger a recompile.
    #[tokio::test]
    async fn watch_task_ignores_non_nx_file_changes() {
        use tokio::time::{sleep, timeout, Duration};

        let tmp = TempDir::new().unwrap();
        make_watch_project(tmp.path());

        let proj = crate::application::project::NexaProject::load(tmp.path()).unwrap();
        let state = Arc::new(nexa_server::AppState::new(
            String::new(),
            String::new(),
            0,
        ));
        let mut rx = state.tx.subscribe();

        let s = state.clone();
        let p = proj.clone();
        tokio::spawn(async move { watch_task(s, p).await });

        sleep(Duration::from_millis(300)).await;

        // Write a non-.nx file inside the watched directory.
        fs::write(
            tmp.path()
                .join("modules")
                .join("core")
                .join("src")
                .join("main")
                .join("notes.md"),
            "should be ignored",
        )
        .unwrap();

        // No reload signal should arrive within 600 ms.
        let result = timeout(Duration::from_millis(600), rx.recv()).await;
        assert!(
            result.is_err(),
            "watch_task must not send a reload signal for non-.nx file changes"
        );
    }

    // ── watch_task: compile error does not crash the task ────────────────────

    /// When a .nx file contains invalid syntax the task must:
    /// - NOT crash (the background task stays alive)
    /// - NOT update AppState (state remains as initially set)
    /// - NOT send a reload signal
    #[tokio::test]
    async fn watch_task_handles_compile_error_without_crashing() {
        use tokio::time::{sleep, timeout, Duration};

        let tmp = TempDir::new().unwrap();
        make_watch_project(tmp.path());

        let proj = crate::application::project::NexaProject::load(tmp.path()).unwrap();
        let sentinel_html = "sentinel-html".to_string();
        let state = Arc::new(nexa_server::AppState::new(
            sentinel_html.clone(),
            String::new(),
            0,
        ));
        let mut rx = state.tx.subscribe();

        let s = state.clone();
        let p = proj.clone();
        tokio::spawn(async move { watch_task(s, p).await });

        sleep(Duration::from_millis(300)).await;

        // Overwrite with syntactically invalid Nexa.
        fs::write(app_nx_path(tmp.path()), "THIS IS NOT VALID NEXA !!!").unwrap();

        // No reload signal should arrive within 800 ms.
        let result = timeout(Duration::from_millis(800), rx.recv()).await;
        assert!(
            result.is_err(),
            "watch_task must not send a reload signal when compilation fails"
        );

        // AppState HTML must not have changed.
        let shared = state.shared.read().await;
        assert_eq!(
            shared.html, sentinel_html,
            "AppState HTML must not change when compilation fails"
        );
    }

    // ── watch_task: subsequent change after error recovers ───────────────────

    /// After a compile error, fixing the source must still trigger a reload.
    /// Verifies the watch loop continues running after an error.
    #[tokio::test]
    async fn watch_task_recovers_after_compile_error() {
        use tokio::time::{sleep, timeout, Duration};

        let tmp = TempDir::new().unwrap();
        make_watch_project(tmp.path());

        let proj = crate::application::project::NexaProject::load(tmp.path()).unwrap();
        let state = Arc::new(nexa_server::AppState::new(
            String::new(),
            String::new(),
            0,
        ));
        let mut rx = state.tx.subscribe();

        let s = state.clone();
        let p = proj.clone();
        tokio::spawn(async move { watch_task(s, p).await });

        sleep(Duration::from_millis(300)).await;

        // 1. Introduce a compile error.
        fs::write(app_nx_path(tmp.path()), "INVALID").unwrap();
        sleep(Duration::from_millis(500)).await;

        // 2. Fix the file — the loop must still be running and detect the fix.
        fs::write(
            app_nx_path(tmp.path()),
            "app App {\n  server { port: 3000; }\n  public window HomePage {\n    public render() => Component {\n      return Page { Text(\"Fixed\") };\n    }\n  }\n  route \"/\" => HomePage;\n}\n",
        )
        .unwrap();

        let msg = timeout(Duration::from_secs(4), rx.recv())
            .await
            .expect("expected a reload signal after fixing compile error")
            .expect("broadcast channel closed unexpectedly");
        assert_eq!(msg, "reload");
    }

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

    // ── load_build_lock ───────────────────────────────────────────────────────

    #[test]
    fn load_build_lock_returns_default_when_missing() {
        let tmp = TempDir::new().unwrap();
        let lock = load_build_lock(tmp.path());
        assert!(lock.modules.is_empty());
        assert!(lock.nexa_version.is_empty());
    }

    #[test]
    fn load_build_lock_reads_existing_file() {
        let tmp = TempDir::new().unwrap();
        let entry = BuildLockEntry { path: "core/app.nx".into(), sha256: "abc".into() };
        save_build_lock(tmp.path(), &[("core", vec![entry.clone()])]);

        let lock = load_build_lock(tmp.path());
        assert!(lock.modules.contains_key("core"));
        assert_eq!(lock.modules["core"][0], entry);
    }

    // ── is_module_up_to_date ──────────────────────────────────────────────────

    #[test]
    fn up_to_date_false_when_no_lock_entry() {
        let tmp = TempDir::new().unwrap();
        // Create a fake app.js so the dist check passes.
        fs::write(tmp.path().join("app.js"), "").unwrap();
        let lock = BuildLockfile::default();
        assert!(!is_module_up_to_date(&lock, "core", &[], tmp.path()));
    }

    #[test]
    fn up_to_date_false_when_dist_missing() {
        let tmp = TempDir::new().unwrap();
        // Lock has an entry but dist/app.js does not exist.
        let entry = BuildLockEntry { path: "app.nx".into(), sha256: "xyz".into() };
        let mut lock = BuildLockfile::default();
        lock.modules.insert("core".into(), vec![entry.clone()]);
        // dist_dir = tmp.path() which has no app.js
        assert!(!is_module_up_to_date(&lock, "core", &[entry], tmp.path()));
    }

    #[test]
    fn up_to_date_false_when_fingerprint_changed() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("app.js"), "").unwrap();
        let old = BuildLockEntry { path: "app.nx".into(), sha256: "old_hash".into() };
        let new = BuildLockEntry { path: "app.nx".into(), sha256: "new_hash".into() };
        let mut lock = BuildLockfile::default();
        lock.modules.insert("core".into(), vec![old]);
        assert!(!is_module_up_to_date(&lock, "core", &[new], tmp.path()));
    }

    #[test]
    fn up_to_date_true_when_fingerprint_matches_and_dist_exists() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("app.js"), "").unwrap();
        let entry = BuildLockEntry { path: "app.nx".into(), sha256: "hash123".into() };
        let mut lock = BuildLockfile::default();
        lock.modules.insert("core".into(), vec![entry.clone()]);
        assert!(is_module_up_to_date(&lock, "core", &[entry], tmp.path()));
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
