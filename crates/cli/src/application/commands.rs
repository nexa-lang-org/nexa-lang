use crate::application::project::NexaProject;
use nexa_compiler::{compile_project_file, compile_to_bundle, decode_nxb, CodeGenerator};
use nexa_server::{AppState, build_router};
use notify::{Config as WatchConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn load_project(dir: Option<PathBuf>) -> NexaProject {
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    NexaProject::load(&dir).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    })
}

pub fn build(project_dir: Option<PathBuf>) {
    let proj = load_project(project_dir);
    println!("Compiling {} ...", proj.entry_file().display());
    match compile_project_file(&proj.entry_file(), &proj.src_root()) {
        Ok(result) => write_dist(&proj.dist_dir(), result),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

pub async fn run(
    bundle: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    port_override: Option<u16>,
    watch: bool,
) {
    // If a .nexa bundle is provided, serve it directly
    if let Some(bundle_path) = bundle {
        if bundle_path.extension().map(|e| e == "nexa").unwrap_or(false) {
            run_from_bundle(bundle_path, port_override).await;
            return;
        } else {
            eprintln!("error: expected a .nexa file, got '{}'", bundle_path.display());
            std::process::exit(1);
        }
    }

    let proj = load_project(project_dir);
    println!("Compiling {} ...", proj.entry_file().display());
    let result = match compile_project_file(&proj.entry_file(), &proj.src_root()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let dist = proj.dist_dir();
    let _ = fs::create_dir_all(&dist);
    let _ = fs::write(dist.join("index.html"), &result.html);
    let _ = fs::write(dist.join("app.js"),     &result.js);

    let port  = port_override.unwrap_or(3000);
    let state = Arc::new(AppState::new(result.html, result.js, port));

    if watch {
        println!("Watch mode — watching {}", proj.src_root().display());
        let state_clone = state.clone();
        let proj_clone  = proj.clone();
        tokio::spawn(async move {
            watch_task(state_clone, proj_clone).await;
        });
    }

    let router   = build_router(state);
    let addr     = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("Cannot bind to {addr}: {e}");
        std::process::exit(1);
    });
    println!("Nexa dev server → http://localhost:{port}");
    axum::serve(listener, router.into_make_service()).await.unwrap();
}

pub fn package(project_dir: Option<PathBuf>, output: Option<PathBuf>) {
    let proj = load_project(project_dir);
    let app_name    = proj.project.name.clone();
    let app_version = proj.project.version.clone();

    println!("Packaging {} v{} ...", app_name, app_version);

    let bundle = match compile_to_bundle(
        &proj.entry_file(),
        &proj.src_root(),
        &app_name,
        &app_version,
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let out_path = output.unwrap_or_else(|| PathBuf::from(format!("{}.nexa", app_name)));

    let file = fs::File::create(&out_path).unwrap_or_else(|e| {
        eprintln!("cannot create {}: {e}", out_path.display());
        std::process::exit(1);
    });

    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("app.nxb", opts).expect("zip: app.nxb");
    zip.write_all(&bundle.nxb).expect("zip: write nxb");

    zip.start_file("manifest.json", opts).expect("zip: manifest.json");
    zip.write_all(bundle.manifest.as_bytes()).expect("zip: write manifest");

    zip.start_file("signature.sig", opts).expect("zip: signature.sig");
    zip.write_all(bundle.signature.as_bytes()).expect("zip: write sig");

    zip.finish().expect("zip: finish");

    println!("Package OK → {}", out_path.display());
}

async fn run_from_bundle(bundle_path: PathBuf, port_override: Option<u16>) {
    println!("Loading bundle {} ...", bundle_path.display());

    let file = fs::File::open(&bundle_path).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", bundle_path.display());
        std::process::exit(1);
    });
    let mut archive = zip::ZipArchive::new(file).unwrap_or_else(|e| {
        eprintln!("invalid .nexa file: {e}");
        std::process::exit(1);
    });

    let nxb_bytes      = read_zip_entry(&mut archive, "app.nxb");
    let manifest_bytes = read_zip_entry(&mut archive, "manifest.json");
    let sig_bytes      = read_zip_entry(&mut archive, "signature.sig");

    // Validate signature
    let expected_sig = String::from_utf8_lossy(&sig_bytes).trim().to_string();
    let mut hasher = Sha256::new();
    hasher.update(&nxb_bytes);
    hasher.update(&manifest_bytes);
    let actual_sig = format!("{:x}", hasher.finalize());

    if actual_sig != expected_sig {
        eprintln!("error: bundle signature validation failed — file may be corrupted or tampered");
        std::process::exit(1);
    }
    println!("Signature OK");

    let program = decode_nxb(&nxb_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to decode bundle: {e}");
        std::process::exit(1);
    });

    let result = CodeGenerator::new().generate(&program).unwrap_or_else(|e| {
        eprintln!("error: codegen failed: {e}");
        std::process::exit(1);
    });

    let port = port_override
        .or_else(|| program.server.as_ref().map(|s| s.port))
        .unwrap_or(3000);

    let state    = Arc::new(AppState::new(result.html, result.js, port));
    let router   = build_router(state);
    let addr     = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("Cannot bind to {addr}: {e}");
        std::process::exit(1);
    });
    println!("Nexa dev server (bundle) → http://localhost:{port}");
    axum::serve(listener, router.into_make_service()).await.unwrap();
}

fn read_zip_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Vec<u8> {
    let mut entry = archive.by_name(name).unwrap_or_else(|_| {
        eprintln!("error: .nexa bundle is missing '{name}'");
        std::process::exit(1);
    });
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).unwrap_or_else(|e| {
        eprintln!("error: failed to read '{name}' from bundle: {e}");
        std::process::exit(1);
    });
    buf
}

/// Watches `src/` for `.nx` changes, recompiles, and broadcasts "reload"
/// to all connected WebSocket clients.
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
    .unwrap_or_else(|e| {
        eprintln!("Watch error: {e}");
        std::process::exit(1);
    });

    watcher
        .watch(&proj.src_root(), RecursiveMode::Recursive)
        .unwrap_or_else(|e| {
            eprintln!("Watch error: {e}");
            std::process::exit(1);
        });

    while let Some(event) = rx.recv().await {
        let has_nx = event
            .paths
            .iter()
            .any(|p| p.extension().map(|e| e == "nx").unwrap_or(false));

        if !has_nx {
            continue;
        }

        println!("Change detected, recompiling...");
        match compile_project_file(&proj.entry_file(), &proj.src_root()) {
            Ok(result) => {
                state.update(result.html, result.js).await;
                println!("Recompile OK — reload sent");
            }
            Err(e) => eprintln!("{e}"),
        }
    }
}

pub fn write_dist(dist_dir: &Path, result: nexa_compiler::CompileResult) {
    fs::create_dir_all(dist_dir).expect("cannot create dist/");
    fs::write(dist_dir.join("index.html"), &result.html).expect("cannot write index.html");
    fs::write(dist_dir.join("app.js"),     &result.js).expect("cannot write app.js");
    println!("Build OK → {}", dist_dir.display());
}
