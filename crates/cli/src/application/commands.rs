use crate::application::{credentials, project::NexaProject, updater};
use nexa_compiler::{compile_project_file, compile_to_bundle, decode_nxb, CodeGenerator};
use nexa_server::{build_router, AppState};
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

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init(name: Option<String>, author: Option<String>, version: String, no_git: bool) {
    // ── resolve project name and target directory ─────────────────────────────
    let project_name = name.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "my-app".to_string())
    });

    // Validate name: only lowercase alphanumerics, hyphens, underscores
    if !project_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        eprintln!(
            "error: project name '{}' must only contain letters, digits, hyphens or underscores",
            project_name
        );
        std::process::exit(1);
    }

    let root = match name {
        Some(_) => PathBuf::from(&project_name),
        None => PathBuf::from("."),
    };

    // ── guard: refuse to clobber an existing project ─────────────────────────
    if root.join("project.json").exists() {
        eprintln!(
            "error: a Nexa project already exists in '{}'",
            root.display()
        );
        eprintln!("       Delete project.json first if you want to reinitialise.");
        std::process::exit(1);
    }

    let author_str = author.unwrap_or_else(|| {
        // Try git config first
        std::process::Command::new("git")
            .args(["config", "--get", "user.name"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown".to_string())
    });

    // ── create directory structure ────────────────────────────────────────────
    let src_main = root.join("src").join("main");
    fs::create_dir_all(&src_main).unwrap_or_else(|e| {
        eprintln!("error: cannot create directory structure: {e}");
        std::process::exit(1);
    });

    // ── project.json ─────────────────────────────────────────────────────────
    let project_json = format!(
        r#"{{
  "name": "{name}",
  "version": "{ver}",
  "author": "{author}",
  "main": "app.nx",
  "dependencies": {{}}
}}
"#,
        name = project_name,
        ver = version,
        author = author_str,
    );
    write_file(&root.join("project.json"), &project_json);

    // ── nexa-compiler.yaml ────────────────────────────────────────────────────
    let compiler_yaml = r#"version: "0.1"
# registry: "https://registry.nexa-lang.org"
# private_registries:
#   - url: "https://corp.registry.example.com"
#     key: "sk_live_..."
"#;
    write_file(&root.join("nexa-compiler.yaml"), compiler_yaml);

    // ── src/main/app.nx  ─────────────────────────────────────────────────────
    let app_class = to_pascal_case(&project_name);
    let app_nx = format!(
        r#"package {pkg};

app {app} {{
  server {{ port: 3000; }}

  public window HomePage {{
    public render() => Component {{
      return Page {{
        Heading("Welcome to {app}!")
      }};
    }}
  }}

  route "/" => HomePage;
}}
"#,
        pkg = project_name.replace('-', "_"),
        app = app_class,
    );
    write_file(&src_main.join("app.nx"), &app_nx);

    // ── .gitignore ────────────────────────────────────────────────────────────
    let gitignore = r#"# Rust build artifacts
/target/

# Nexa compiler output
dist/
**/src/dist/
**/src/.nexa/

# Installed packages
nexa-libs/

# Distributable bundles (built by nexa package)
*.nexa

# macOS
.DS_Store

# Environment variables
.env
"#;
    write_file(&root.join(".gitignore"), gitignore);

    // ── git init ──────────────────────────────────────────────────────────────
    let git_initted = if !no_git {
        let ok = std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ok
    } else {
        false
    };

    // ── success message ───────────────────────────────────────────────────────
    println!();
    println!(
        "  \x1b[1;32m✓\x1b[0m  Created Nexa project \x1b[1m{}\x1b[0m",
        project_name
    );
    println!();
    println!("  \x1b[2m{}/\x1b[0m", root.display());
    println!("  \x1b[2m├── project.json\x1b[0m");
    println!("  \x1b[2m├── nexa-compiler.yaml\x1b[0m");
    println!("  \x1b[2m├── .gitignore\x1b[0m");
    println!("  \x1b[2m└── src/main/app.nx\x1b[0m");
    if git_initted {
        println!("  \x1b[2m   (git repository initialised)\x1b[0m");
    }
    println!();

    let cd_hint = if root == Path::new(".") {
        String::new()
    } else {
        format!("  cd {}\n", project_name)
    };

    println!("  Next steps:");
    println!();
    print!("{}", cd_hint);
    println!("  \x1b[1mnexa run\x1b[0m          # start the dev server on http://localhost:3000");
    println!("  \x1b[1mnexa run --watch\x1b[0m   # with hot reload");
    println!("  \x1b[1mnexa build\x1b[0m         # compile to src/dist/");
    println!();
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).unwrap_or_else(|e| {
        eprintln!("error: cannot write {}: {e}", path.display());
        std::process::exit(1);
    });
}

/// "my-cool-app" → "MyCoolApp"
fn to_pascal_case(s: &str) -> String {
    s.split(['-', '_'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

#[cfg(test)]
mod init_tests {
    use super::to_pascal_case;

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(to_pascal_case("my-app"), "MyApp");
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
        assert_eq!(to_pascal_case("my-cool-app"), "MyCoolApp");
        assert_eq!(to_pascal_case("app"), "App");
        assert_eq!(to_pascal_case("a-b-c"), "ABC");
        assert_eq!(to_pascal_case(""), "");
    }
}

// ── Build ─────────────────────────────────────────────────────────────────────

pub fn build(project_dir: Option<PathBuf>) {
    updater::check_and_notify("stable");
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
        if bundle_path
            .extension()
            .map(|e| e == "nexa")
            .unwrap_or(false)
        {
            run_from_bundle(bundle_path, port_override).await;
            return;
        } else {
            eprintln!(
                "error: expected a .nexa file, got '{}'",
                bundle_path.display()
            );
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
    let _ = fs::write(dist.join("app.js"), &result.js);

    let port = port_override.unwrap_or(3000);
    let state = Arc::new(AppState::new(result.html, result.js, port));

    if watch {
        println!("Watch mode — watching {}", proj.src_root().display());
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
            eprintln!("Cannot bind to {addr}: {e}");
            std::process::exit(1);
        });
    println!("Nexa dev server → http://localhost:{port}");
    axum::serve(listener, router.into_make_service())
        .await
        .unwrap();
}

pub fn package(project_dir: Option<PathBuf>, output: Option<PathBuf>) {
    let proj = load_project(project_dir);
    let app_name = proj.project.name.clone();
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
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("app.nxb", opts).expect("zip: app.nxb");
    zip.write_all(&bundle.nxb).expect("zip: write nxb");

    zip.start_file("manifest.json", opts)
        .expect("zip: manifest.json");
    zip.write_all(bundle.manifest.as_bytes())
        .expect("zip: write manifest");

    zip.start_file("signature.sig", opts)
        .expect("zip: signature.sig");
    zip.write_all(bundle.signature.as_bytes())
        .expect("zip: write sig");

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

    let nxb_bytes = read_zip_entry(&mut archive, "app.nxb");
    let manifest_bytes = read_zip_entry(&mut archive, "manifest.json");
    let sig_bytes = read_zip_entry(&mut archive, "signature.sig");

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

    let state = Arc::new(AppState::new(result.html, result.js, port));
    let router = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Cannot bind to {addr}: {e}");
            std::process::exit(1);
        });
    println!("Nexa dev server (bundle) → http://localhost:{port}");
    axum::serve(listener, router.into_make_service())
        .await
        .unwrap();
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

// ── Registry commands ─────────────────────────────────────────────────────────

const DEFAULT_REGISTRY: &str = "https://registry.nexa-lang.org";

pub fn register(registry_override: Option<String>) {
    let registry = registry_override.unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let (email, password) = prompt_credentials();
    println!("Registering on {} ...", registry);
    let url = format!("{registry}/auth/register");
    match post_json(
        &url,
        &serde_json::json!({ "email": email, "password": password }),
        None,
    ) {
        Ok(body) => {
            if let Some(token) = body["token"].as_str() {
                credentials::save(&registry, token);
                println!("Account created. Logged in as {email}");
            } else {
                eprintln!(
                    "error: {}",
                    body["error"].as_str().unwrap_or("unknown error")
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn login(registry_override: Option<String>) {
    let registry = registry_override.unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let (email, password) = prompt_credentials();
    println!("Logging in to {} ...", registry);
    let url = format!("{registry}/auth/login");
    match post_json(
        &url,
        &serde_json::json!({ "email": email, "password": password }),
        None,
    ) {
        Ok(body) => {
            if let Some(token) = body["token"].as_str() {
                credentials::save(&registry, token);
                println!("Logged in as {email}");
            } else {
                eprintln!(
                    "error: {}",
                    body["error"].as_str().unwrap_or("invalid credentials")
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn publish(project_dir: Option<PathBuf>, registry_override: Option<String>) {
    let proj = load_project(project_dir);
    let app_name = proj.project.name.clone();
    let app_version = proj.project.version.clone();

    let creds = credentials::load().unwrap_or_else(|| {
        eprintln!("error: not logged in. Run `nexa login` first.");
        std::process::exit(1);
    });
    let registry = registry_override.unwrap_or(creds.registry.clone());

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

    // Write bundle to a temp file
    let tmp_path = std::env::temp_dir().join(format!("{app_name}-{app_version}.nexa"));
    {
        use std::io::Write as _;
        let file = fs::File::create(&tmp_path).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(1);
        });
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("app.nxb", opts).unwrap();
        zip.write_all(&bundle.nxb).unwrap();
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(bundle.manifest.as_bytes()).unwrap();
        zip.start_file("signature.sig", opts).unwrap();
        zip.write_all(bundle.signature.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    println!("Publishing {app_name}@{app_version} to {registry} ...");
    let url = format!("{registry}/packages/{app_name}/publish");
    let file_bytes = fs::read(&tmp_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let _ = fs::remove_file(&tmp_path);

    let client = reqwest::blocking::Client::new();
    let part = reqwest::blocking::multipart::Part::bytes(file_bytes)
        .file_name(format!("{app_name}.nexa"))
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::blocking::multipart::Form::new().part("file", part);

    match client
        .post(&url)
        .bearer_auth(&creds.token)
        .multipart(form)
        .send()
    {
        Ok(resp) if resp.status().is_success() => {
            println!("Published {app_name}@{app_version}");
        }
        Ok(resp) => {
            let body: serde_json::Value = resp.json().unwrap_or_default();
            eprintln!(
                "error: {}",
                body["error"].as_str().unwrap_or("publish failed")
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn install(package_arg: Option<String>, project_dir: Option<PathBuf>) {
    let proj = load_project(project_dir);
    let registries = proj.compiler.all_registries();

    let packages_to_install: Vec<(String, String)> = if let Some(arg) = package_arg {
        // Parse "name" or "name@version"
        if let Some((name, ver)) = arg.split_once('@') {
            vec![(name.to_string(), ver.to_string())]
        } else {
            vec![(arg, "latest".to_string())]
        }
    } else {
        // Install all deps from project.json
        proj.project
            .dependencies
            .iter()
            .map(|(name, ver)| (name.clone(), ver.trim_start_matches('^').to_string()))
            .collect()
    };

    if packages_to_install.is_empty() {
        println!("No dependencies to install.");
        return;
    }

    let libs_dir = proj.libs_dir();
    fs::create_dir_all(&libs_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create nexa-libs/: {e}");
        std::process::exit(1);
    });

    let mut lock = load_lockfile(&libs_dir);

    for (name, version) in &packages_to_install {
        println!("Installing {name}@{version} ...");
        let bundle = try_download(&registries, name, version);
        let (registry_url, bundle_bytes) = bundle.unwrap_or_else(|| {
            eprintln!("error: package {name}@{version} not found in any registry");
            std::process::exit(1);
        });

        // Verify signature
        verify_bundle_signature(&bundle_bytes, name);

        // Extract to nexa-libs/<name>@<version>/
        let pkg_dir = libs_dir.join(format!("{name}@{version}"));
        extract_bundle_to(&bundle_bytes, &pkg_dir);

        // Read actual version from manifest (in case "latest" was used)
        let manifest_path = pkg_dir.join("manifest.json");
        let resolved_version = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["version"].as_str().map(String::from))
            .unwrap_or_else(|| version.clone());

        let sig = fs::read_to_string(pkg_dir.join("signature.sig")).unwrap_or_default();
        lock.packages.retain(|p: &LockEntry| p.name != *name);
        lock.packages.push(LockEntry {
            name: name.clone(),
            version: resolved_version,
            registry: registry_url,
            signature: sig.trim().to_string(),
        });

        println!("  ✓ {name}");
    }

    save_lockfile(&libs_dir, &lock);
    println!("Done. {} package(s) installed.", packages_to_install.len());
}

// ── Registry helpers ──────────────────────────────────────────────────────────

fn prompt_credentials() -> (String, String) {
    use std::io::{self, BufRead};
    print!("Email: ");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let stdin = io::stdin();
    let email = stdin
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    let password = rpassword::prompt_password("Password: ").unwrap_or_default();
    (email, password)
}

fn post_json(
    url: &str,
    body: &serde_json::Value,
    token: Option<&str>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::new();
    let mut req = client.post(url).json(body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    resp.json::<serde_json::Value>().map_err(|e| e.to_string())
}

fn try_download(
    registries: &[(String, Option<String>)],
    name: &str,
    version: &str,
) -> Option<(String, Vec<u8>)> {
    let client = reqwest::blocking::Client::new();
    for (url, key) in registries {
        let endpoint = format!("{url}/packages/{name}/{version}/download");
        let mut req = client.get(&endpoint);
        if let Some(k) = key {
            req = req.header("X-Api-Key", k);
        }
        if let Ok(resp) = req.send() {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes() {
                    return Some((url.clone(), bytes.to_vec()));
                }
            }
        }
    }
    None
}

fn verify_bundle_signature(bundle: &[u8], name: &str) {
    use std::io::{Cursor, Read as _};
    let cursor = Cursor::new(bundle);
    let mut archive = zip::ZipArchive::new(cursor).unwrap_or_else(|e| {
        eprintln!("error: invalid bundle for {name}: {e}");
        std::process::exit(1);
    });

    let nxb = {
        let mut e = archive.by_name("app.nxb").unwrap_or_else(|_| {
            eprintln!("error: bundle missing app.nxb");
            std::process::exit(1);
        });
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).unwrap();
        buf
    };
    let manifest_str = {
        let mut e = archive.by_name("manifest.json").unwrap_or_else(|_| {
            eprintln!("error: bundle missing manifest.json");
            std::process::exit(1);
        });
        let mut buf = String::new();
        e.read_to_string(&mut buf).unwrap();
        buf
    };
    let sig_str = {
        let mut e = archive.by_name("signature.sig").unwrap_or_else(|_| {
            eprintln!("error: bundle missing signature.sig");
            std::process::exit(1);
        });
        let mut buf = String::new();
        e.read_to_string(&mut buf).unwrap();
        buf
    };

    let mut hasher = Sha256::new();
    hasher.update(&nxb);
    hasher.update(manifest_str.as_bytes());
    let computed = format!("{:x}", hasher.finalize());
    if computed != sig_str.trim() {
        eprintln!("error: signature verification failed for {name} — bundle may be corrupted");
        std::process::exit(1);
    }
}

fn extract_bundle_to(bundle: &[u8], dest: &Path) {
    use std::io::{Cursor, Read as _};
    fs::create_dir_all(dest).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {e}", dest.display());
        std::process::exit(1);
    });
    let cursor = Cursor::new(bundle);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let out_path = dest.join(entry.name());
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        fs::write(&out_path, &buf).unwrap_or_else(|e| {
            eprintln!("error: write {}: {e}", out_path.display());
            std::process::exit(1);
        });
    }
}

// ── Lockfile ──────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Lockfile {
    packages: Vec<LockEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LockEntry {
    name: String,
    version: String,
    registry: String,
    signature: String,
}

fn load_lockfile(libs_dir: &Path) -> Lockfile {
    let path = libs_dir.join(".lock");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_lockfile(libs_dir: &Path, lock: &Lockfile) {
    let path = libs_dir.join(".lock");
    let json = serde_json::to_string_pretty(lock).expect("serialize lockfile");
    fs::write(&path, json).unwrap_or_else(|e| {
        eprintln!("warning: could not write lockfile: {e}");
    });
}

// ── Build / Run / Package (unchanged) ─────────────────────────────────────────

pub fn write_dist(dist_dir: &Path, result: nexa_compiler::CompileResult) {
    fs::create_dir_all(dist_dir).expect("cannot create dist/");
    fs::write(dist_dir.join("index.html"), &result.html).expect("cannot write index.html");
    fs::write(dist_dir.join("app.js"), &result.js).expect("cannot write app.js");
    println!("Build OK → {}", dist_dir.display());
}

// ── Update ────────────────────────────────────────────────────────────────────

pub fn update(channel_override: Option<String>) {
    updater::run_update_command(channel_override);
}
