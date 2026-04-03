use crate::application::{config, credentials, project::NexaProject, updater};
use crate::infrastructure::ui;
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
    NexaProject::load(&dir).unwrap_or_else(|e| ui::die(e.to_string()))
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init(name: Option<String>, author: Option<String>, version: String, no_git: bool) {
    let project_name = name.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "my-app".to_string())
    });

    if !project_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        ui::die(format!(
            "project name '{}' must only contain letters, digits, hyphens or underscores",
            project_name
        ));
    }

    let root = match name {
        Some(_) => PathBuf::from(&project_name),
        None => PathBuf::from("."),
    };

    if root.join("project.json").exists() {
        ui::die(format!(
            "a Nexa project already exists in '{}'\n  Delete project.json first if you want to reinitialise.",
            root.display()
        ));
    }

    let author_str = author.unwrap_or_else(|| {
        std::process::Command::new("git")
            .args(["config", "--get", "user.name"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown".to_string())
    });

    let src_main = root.join("src").join("main");
    fs::create_dir_all(&src_main)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory structure: {e}")));

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

    let compiler_yaml = r#"version: "0.1"
# registry: "https://registry.nexa-lang.org"
# private_registries:
#   - url: "https://corp.registry.example.com"
#     key: "sk_live_..."
"#;
    write_file(&root.join("nexa-compiler.yaml"), compiler_yaml);

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

    let gitignore = r#"# Nexa compiler output
dist/
**/src/dist/
**/src/.nexa/

# Installed packages
nexa-libs/

# Distributable bundles
*.nexa

# macOS
.DS_Store

# Environment variables
.env
"#;
    write_file(&root.join(".gitignore"), gitignore);

    let git_initted = if !no_git {
        std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        false
    };

    ui::blank();
    ui::success(format!(
        "Created Nexa project \x1b[1m{}\x1b[0m",
        project_name
    ));
    ui::blank();
    ui::hint(format!("  {}/", root.display()));
    ui::hint("  ├── project.json");
    ui::hint("  ├── nexa-compiler.yaml");
    ui::hint("  ├── .gitignore");
    ui::hint("  └── src/main/app.nx");
    if git_initted {
        ui::hint("      (git repository initialised)");
    }
    ui::blank();

    let cd_hint = if root != Path::new(".") {
        format!("  cd {project_name}\n")
    } else {
        String::new()
    };

    println!("  Next steps:\n");
    print!("{cd_hint}");
    println!("  \x1b[1mnexa run\x1b[0m           start the dev server on http://localhost:3000");
    println!("  \x1b[1mnexa run --watch\x1b[0m   with hot reload");
    println!("  \x1b[1mnexa build\x1b[0m         compile to src/dist/");
    ui::blank();
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content)
        .unwrap_or_else(|e| ui::die(format!("cannot write {}: {e}", path.display())));
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
    let sp = ui::spinner(format!("Compiling {}…", proj.entry_file().display()));
    match compile_project_file(&proj.entry_file(), &proj.src_root()) {
        Ok(result) => {
            write_dist(&proj.dist_dir(), result);
            ui::done(&sp, format!("Build OK  →  {}", proj.dist_dir().display()));
        }
        Err(e) => ui::fail(&sp, e.to_string()),
    }
}

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
    let sp = ui::spinner(format!("Compiling {}…", proj.entry_file().display()));
    let result = match compile_project_file(&proj.entry_file(), &proj.src_root()) {
        Ok(r) => r,
        Err(e) => ui::fail(&sp, e.to_string()),
    };
    ui::done(&sp, "Compiled");

    let dist = proj.dist_dir();
    let _ = fs::create_dir_all(&dist);
    let _ = fs::write(dist.join("index.html"), &result.html);
    let _ = fs::write(dist.join("app.js"), &result.js);

    let port = port_override.unwrap_or(3000);
    let state = Arc::new(AppState::new(result.html, result.js, port));

    if watch {
        ui::info(format!("Watch mode  →  {}", proj.src_root().display()));
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

pub fn package(project_dir: Option<PathBuf>, output: Option<PathBuf>) {
    let proj = load_project(project_dir);
    let app_name = proj.project.name.clone();
    let app_version = proj.project.version.clone();

    let sp = ui::spinner(format!("Packaging {app_name} v{app_version}…"));

    let bundle = match compile_to_bundle(
        &proj.entry_file(),
        &proj.src_root(),
        &app_name,
        &app_version,
    ) {
        Ok(b) => b,
        Err(e) => ui::fail(&sp, e.to_string()),
    };

    let out_path = output.unwrap_or_else(|| PathBuf::from(format!("{app_name}.nexa")));
    let file = fs::File::create(&out_path)
        .unwrap_or_else(|e| ui::fail(&sp, format!("cannot create {}: {e}", out_path.display())));

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
    // Include entry source so the registry and CLI can display it
    let src_entry = format!("src/{}", bundle.source_filename);
    zip.start_file(&src_entry, opts).expect("zip: src");
    zip.write_all(bundle.source.as_bytes())
        .expect("zip: write src");
    zip.finish().expect("zip: finish");

    ui::done(&sp, format!("Package OK  →  {}", out_path.display()));
}

async fn run_from_bundle(bundle_path: PathBuf, port_override: Option<u16>) {
    let sp = ui::spinner(format!("Loading bundle {}…", bundle_path.display()));

    let file = fs::File::open(&bundle_path)
        .unwrap_or_else(|e| ui::fail(&sp, format!("cannot open {}: {e}", bundle_path.display())));
    let mut archive = zip::ZipArchive::new(file)
        .unwrap_or_else(|e| ui::fail(&sp, format!("invalid .nexa file: {e}")));

    let nxb_bytes = read_zip_entry(&mut archive, "app.nxb");
    let manifest_bytes = read_zip_entry(&mut archive, "manifest.json");
    let sig_bytes = read_zip_entry(&mut archive, "signature.sig");

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

fn read_zip_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Vec<u8> {
    let mut entry = archive.by_name(name).unwrap_or_else(|_| {
        ui::die(format!(".nexa bundle is missing '{name}'"));
    });
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).unwrap_or_else(|e| {
        ui::die(format!("failed to read '{name}' from bundle: {e}"));
    });
    buf
}

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
        .watch(&proj.src_root(), RecursiveMode::Recursive)
        .unwrap_or_else(|e| ui::die(format!("Watch error: {e}")));

    while let Some(event) = rx.recv().await {
        let has_nx = event
            .paths
            .iter()
            .any(|p| p.extension().map(|e| e == "nx").unwrap_or(false));

        if !has_nx {
            continue;
        }

        let sp = ui::spinner("Recompiling…");
        match compile_project_file(&proj.entry_file(), &proj.src_root()) {
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

// ── Registry commands ─────────────────────────────────────────────────────────

const DEFAULT_REGISTRY: &str = "https://registry.nexa-lang.org";

pub fn register(registry_override: Option<String>) {
    let registry = registry_override
        .or_else(|| Some(config::load().registry))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    ui::header("Create account");
    let email = ui::input("Email", None);
    let password = ui::password("Password");

    let sp = ui::spinner(format!("Creating account on {registry}…"));
    let url = format!("{registry}/auth/register");
    match post_json(
        &url,
        &serde_json::json!({ "email": email, "password": password }),
        None,
    ) {
        Ok(body) => {
            if let Some(token) = body["token"].as_str() {
                credentials::save(&registry, token);
                ui::done(&sp, format!("Account created  ·  logged in as {email}"));
            } else {
                ui::fail(&sp, body["error"].as_str().unwrap_or("unknown error"));
            }
        }
        Err(e) => ui::fail(&sp, e),
    }
}

pub fn login(registry_override: Option<String>) {
    let registry = registry_override
        .or_else(|| Some(config::load().registry))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    ui::header("Login");
    let email = ui::input("Email", None);
    let password = ui::password("Password");

    let sp = ui::spinner(format!("Authenticating with {registry}…"));
    let url = format!("{registry}/auth/login");
    match post_json(
        &url,
        &serde_json::json!({ "email": email, "password": password }),
        None,
    ) {
        Ok(body) => {
            if let Some(token) = body["token"].as_str() {
                credentials::save(&registry, token);
                ui::done(&sp, format!("Logged in as {email}"));
            } else {
                ui::fail(&sp, body["error"].as_str().unwrap_or("invalid credentials"));
            }
        }
        Err(e) => ui::fail(&sp, e),
    }
}

// ── API token commands ────────────────────────────────────────────────────────

pub fn token_create(name: String, registry_override: Option<String>) {
    let creds = credentials::load().unwrap_or_else(|| {
        ui::die("not logged in. Run `nexa login` first.");
    });
    let registry = registry_override.unwrap_or(creds.registry.clone());

    let sp = ui::spinner(format!("Creating token '{name}' on {registry}…"));
    let url = format!("{registry}/auth/tokens");
    match post_json(
        &url,
        &serde_json::json!({ "name": name }),
        Some(&creds.token),
    ) {
        Ok(body) if body.get("token").is_some() => {
            ui::done(&sp, format!("Token created  ·  name: {name}"));
            ui::blank();
            println!(
                "  \x1b[1;33mtoken:\x1b[0m  {}",
                body["token"].as_str().unwrap_or("?")
            );
            println!("  \x1b[2mid:    {}\x1b[0m", body["id"].as_str().unwrap_or("?"));
            ui::blank();
            ui::warn("Save this token now — it will not be shown again.");
            ui::blank();
        }
        Ok(body) => {
            ui::fail(&sp, body["error"].as_str().unwrap_or("unknown error"));
        }
        Err(e) => ui::fail(&sp, e),
    }
}

pub fn token_list(registry_override: Option<String>) {
    let creds = credentials::load().unwrap_or_else(|| {
        ui::die("not logged in. Run `nexa login` first.");
    });
    let registry = registry_override.unwrap_or(creds.registry.clone());

    let sp = ui::spinner(format!("Fetching tokens from {registry}…"));
    let client = reqwest::blocking::Client::new();
    let url = format!("{registry}/auth/tokens");
    let result = client
        .get(&url)
        .bearer_auth(&creds.token)
        .send()
        .and_then(|r| r.json::<serde_json::Value>());

    sp.finish_and_clear();

    match result {
        Ok(body) => {
            let tokens = body.as_array().cloned().unwrap_or_default();
            if tokens.is_empty() {
                ui::info("No API tokens yet. Create one with: nexa token create <name>");
            } else {
                ui::header("API tokens");
                let mut table = ui::Table::new(vec!["ID", "Name", "Created", "Last used"]);
                for t in &tokens {
                    let id = t["id"].as_str().unwrap_or("?");
                    let name = t["name"].as_str().unwrap_or("?");
                    let created = t["created_at"].as_str().unwrap_or("?");
                    let last_used = t["last_used_at"].as_str().unwrap_or("never");
                    table.row(vec![
                        id.to_string(),
                        name.to_string(),
                        created.to_string(),
                        last_used.to_string(),
                    ]);
                }
                table.print();
                ui::blank();
                ui::hint("  Revoke:  nexa token revoke <id>");
                ui::blank();
            }
        }
        Err(e) => ui::die(format!("could not fetch tokens: {e}")),
    }
}

pub fn token_revoke(id: String, registry_override: Option<String>) {
    let creds = credentials::load().unwrap_or_else(|| {
        ui::die("not logged in. Run `nexa login` first.");
    });
    let registry = registry_override.unwrap_or(creds.registry.clone());

    if !ui::confirm(&format!("Revoke token {id}?"), false) {
        return;
    }

    let sp = ui::spinner(format!("Revoking token {id}…"));
    let client = reqwest::blocking::Client::new();
    let url = format!("{registry}/auth/tokens/{id}");
    match client.delete(&url).bearer_auth(&creds.token).send() {
        Ok(resp) if resp.status() == 204 => {
            ui::done(&sp, format!("Token {id} revoked."));
        }
        Ok(resp) if resp.status() == 404 => {
            ui::fail(&sp, "token not found");
        }
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().unwrap_or_default();
            ui::fail(
                &sp,
                body["error"]
                    .as_str()
                    .unwrap_or(&format!("HTTP {status}")),
            );
        }
        Err(e) => ui::fail(&sp, e.to_string()),
    }
}

pub fn publish(project_dir: Option<PathBuf>, registry_override: Option<String>) {
    let proj = load_project(project_dir);
    let app_name = proj.project.name.clone();
    let app_version = proj.project.version.clone();

    let creds = credentials::load().unwrap_or_else(|| {
        ui::die("not logged in. Run `nexa login` first.");
    });
    let registry = registry_override.unwrap_or(creds.registry.clone());

    let sp = ui::spinner(format!("Packaging {app_name} v{app_version}…"));
    let bundle = match compile_to_bundle(
        &proj.entry_file(),
        &proj.src_root(),
        &app_name,
        &app_version,
    ) {
        Ok(b) => b,
        Err(e) => ui::fail(&sp, e.to_string()),
    };

    let tmp_path = std::env::temp_dir().join(format!("{app_name}-{app_version}.nexa"));
    {
        use std::io::Write as _;
        let file = fs::File::create(&tmp_path).unwrap_or_else(|e| ui::fail(&sp, e.to_string()));
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("app.nxb", opts).unwrap();
        zip.write_all(&bundle.nxb).unwrap();
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(bundle.manifest.as_bytes()).unwrap();
        zip.start_file("signature.sig", opts).unwrap();
        zip.write_all(bundle.signature.as_bytes()).unwrap();
        let src_entry = format!("src/{}", bundle.source_filename);
        zip.start_file(&src_entry, opts).unwrap();
        zip.write_all(bundle.source.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    sp.set_message(format!(
        "Publishing {app_name}@{app_version} to {registry}…"
    ));

    let url = format!("{registry}/packages/{app_name}/publish");
    let file_bytes = fs::read(&tmp_path).unwrap_or_else(|e| ui::fail(&sp, e.to_string()));
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
            ui::done(&sp, format!("Published {app_name}@{app_version}"));
        }
        Ok(resp) => {
            let body: serde_json::Value = resp.json().unwrap_or_default();
            ui::fail(&sp, body["error"].as_str().unwrap_or("publish failed"));
        }
        Err(e) => ui::fail(&sp, e.to_string()),
    }
}

pub fn install(package_arg: Option<String>, project_dir: Option<PathBuf>) {
    let proj = load_project(project_dir);
    let registries = proj.compiler.all_registries();

    let packages_to_install: Vec<(String, String)> = if let Some(arg) = package_arg {
        if let Some((name, ver)) = arg.split_once('@') {
            vec![(name.to_string(), ver.to_string())]
        } else {
            vec![(arg, "latest".to_string())]
        }
    } else {
        proj.project
            .dependencies
            .iter()
            .map(|(name, ver)| (name.clone(), ver.trim_start_matches('^').to_string()))
            .collect()
    };

    if packages_to_install.is_empty() {
        ui::info("No dependencies to install.");
        return;
    }

    let libs_dir = proj.libs_dir();
    fs::create_dir_all(&libs_dir)
        .unwrap_or_else(|e| ui::die(format!("cannot create nexa-libs/: {e}")));

    let mut lock = load_lockfile(&libs_dir);

    for (name, version) in &packages_to_install {
        let sp = ui::spinner(format!("Installing {name}@{version}…"));
        let bundle = try_download(&registries, name, version);
        let (registry_url, bundle_bytes) = bundle.unwrap_or_else(|| {
            ui::fail(
                &sp,
                format!("package {name}@{version} not found in any registry"),
            )
        });

        verify_bundle_signature(&bundle_bytes, name);

        let pkg_dir = libs_dir.join(format!("{name}@{version}"));
        extract_bundle_to(&bundle_bytes, &pkg_dir);

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
            version: resolved_version.clone(),
            registry: registry_url,
            signature: sig.trim().to_string(),
        });

        ui::done(&sp, format!("{name}@{resolved_version}"));
    }

    save_lockfile(&libs_dir, &lock);

    // Update project.json dependencies with installed packages
    update_project_dependencies(proj.root(), &packages_to_install, &lock);

    ui::blank();
    ui::success(format!(
        "{} package(s) installed.",
        packages_to_install.len()
    ));
}

// ── Search ────────────────────────────────────────────────────────────────────

pub fn search(query: Option<String>, registry_override: Option<String>, limit: u32) {
    let registry = registry_override
        .or_else(|| Some(config::load().registry))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    let q = query.clone().unwrap_or_default();
    let sp = ui::spinner(format!("Searching {registry}…"));

    let url = format!("{registry}/packages?q={q}&per_page={limit}");
    let result = reqwest::blocking::get(&url).and_then(|r| r.json::<serde_json::Value>());

    sp.finish_and_clear();

    match result {
        Ok(body) => {
            let packages = body.as_array().cloned().unwrap_or_default();
            if packages.is_empty() {
                ui::blank();
                ui::info(if q.is_empty() {
                    "No packages found on the registry.".to_string()
                } else {
                    format!("No packages found for '{q}'.")
                });
                ui::blank();
                return;
            }

            ui::blank();
            if q.is_empty() {
                println!("  Packages on \x1b[1m{registry}\x1b[0m\n");
            } else {
                println!("  Results for \x1b[1m\"{q}\"\x1b[0m on {registry}\n");
            }

            let mut table = ui::Table::new(vec!["Package", "Description"]);
            for pkg in &packages {
                let name = pkg["name"].as_str().unwrap_or("?").to_string();
                let desc = pkg["description"].as_str().unwrap_or("—").to_string();
                table.row(vec![name, desc]);
            }
            table.print();

            ui::blank();
            ui::hint(format!(
                "  {} result(s)  ·  install: nexa install <name>",
                packages.len()
            ));
            ui::blank();
        }
        Err(e) => ui::die(format!("search failed: {e}")),
    }
}

// ── Info ──────────────────────────────────────────────────────────────────────

pub fn info(package: String, registry_override: Option<String>) {
    let registry = registry_override
        .or_else(|| Some(config::load().registry))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    let sp = ui::spinner(format!("Fetching info for {package}…"));
    let url = format!("{registry}/packages/{package}");
    let result = reqwest::blocking::get(&url).and_then(|r| r.json::<serde_json::Value>());

    sp.finish_and_clear();

    match result {
        Ok(body) => {
            if body.get("error").is_some() {
                ui::die(format!("package '{package}' not found on {registry}"));
            }

            ui::blank();
            println!(
                "  \x1b[1;36m{}\x1b[0m",
                body["name"].as_str().unwrap_or(&package)
            );
            ui::blank();

            let versions = body["versions"].as_array().cloned().unwrap_or_default();
            if versions.is_empty() {
                ui::info("No versions published yet.");
            } else {
                let mut table = ui::Table::new(vec!["Version", "Published"]);
                for v in &versions {
                    let ver = v["version"].as_str().unwrap_or("?").to_string();
                    let published = v["published_at"].as_str().unwrap_or("—").to_string();
                    table.row(vec![ver, published]);
                }
                table.print();
            }

            ui::blank();
            let latest = versions
                .last()
                .and_then(|v| v["version"].as_str())
                .unwrap_or("latest");
            ui::hint(format!("  Install:  nexa install {package}@{latest}"));
            ui::blank();
        }
        Err(e) => ui::die(format!("could not fetch package info: {e}")),
    }
}

// ── Config commands ───────────────────────────────────────────────────────────

pub fn config_list() {
    ui::header("Nexa CLI configuration");
    for key in config::KEYS {
        let val = config::get(key).unwrap_or_default();
        ui::kv(key, val);
    }
    ui::blank();
    ui::hint(format!(
        "  Config file: {}",
        config::config_path().display()
    ));
    ui::blank();
}

pub fn config_get(key: String) {
    match config::get(&key) {
        Some(val) => println!("{val}"),
        None => ui::die(format!(
            "unknown key '{key}'. Available: {}",
            config::KEYS.join(", ")
        )),
    }
}

pub fn config_set(key: String, value: String) {
    match config::set(&key, &value) {
        Ok(()) => ui::success(format!("{key}  =  {value}")),
        Err(e) => ui::die(e),
    }
}

// ── Theme commands ────────────────────────────────────────────────────────────

pub fn theme_list() {
    let active = config::active_theme();
    let installed = config::list_themes();

    ui::header("Installed themes");

    if installed.is_empty() {
        ui::info("No themes installed.");
        ui::blank();
        ui::hint("  Install a theme:  nexa theme add <name>");
    } else {
        for theme in &installed {
            if theme == &active {
                println!("  \x1b[1;32m●\x1b[0m  \x1b[1m{theme}\x1b[0m  \x1b[2m(active)\x1b[0m");
            } else {
                println!("  \x1b[2m○\x1b[0m  {theme}");
            }
        }
        ui::blank();
        ui::hint("  Activate:  nexa config set theme <name>");
    }
    ui::blank();
}

pub fn theme_add(name: String, registry_override: Option<String>) {
    let registry = registry_override
        .or_else(|| Some(config::load().registry))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    let themes_dir = config::themes_dir();
    let theme_dir = themes_dir.join(&name);

    if theme_dir.exists() {
        if !ui::confirm(
            &format!("Theme '{name}' is already installed. Reinstall?"),
            false,
        ) {
            return;
        }
        let _ = fs::remove_dir_all(&theme_dir);
    }

    let sp = ui::spinner(format!("Downloading theme {name} from {registry}…"));

    // Themes are packages — download from registry like any package
    let registries = vec![(registry.clone(), None::<String>)];
    let bundle = try_download(&registries, &name, "latest");

    let (_, bundle_bytes) =
        bundle.unwrap_or_else(|| ui::fail(&sp, format!("theme '{name}' not found on {registry}")));

    fs::create_dir_all(&theme_dir)
        .unwrap_or_else(|e| ui::fail(&sp, format!("cannot create theme directory: {e}")));

    extract_bundle_to(&bundle_bytes, &theme_dir);

    ui::done(
        &sp,
        format!("Theme '{name}' installed  →  activate with: nexa config set theme {name}"),
    );
}

pub fn theme_remove(name: String) {
    let theme_dir = config::themes_dir().join(&name);
    if !theme_dir.exists() {
        ui::die(format!("theme '{name}' is not installed."));
    }

    if !ui::confirm(&format!("Remove theme '{name}'?"), true) {
        return;
    }

    fs::remove_dir_all(&theme_dir)
        .unwrap_or_else(|e| ui::die(format!("could not remove theme: {e}")));

    // If this was the active theme, reset to default
    if config::active_theme() == name {
        let _ = config::set("theme", "default");
    }

    ui::success(format!("Theme '{name}' removed."));
}

// ── Doctor ────────────────────────────────────────────────────────────────────

pub fn doctor() {
    ui::header("Nexa environment check");

    // Registry connectivity
    let registry = config::load().registry;
    let sp = ui::spinner(format!("Checking registry ({registry})…"));
    let ok = reqwest::blocking::get(format!("{registry}/health"))
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if ok {
        ui::done(&sp, format!("Registry reachable  →  {registry}"));
    } else {
        sp.finish_and_clear();
        ui::warn(format!("Registry unreachable: {registry}"));
    }

    // Auth
    match credentials::load() {
        Some(c) => ui::success(format!("Logged in  →  {}", c.registry)),
        None => ui::warn("Not logged in  ·  run: nexa login"),
    }

    // Config
    ui::success(format!("Config  →  {}", config::config_path().display()));

    // Themes dir
    let themes_count = config::list_themes().len();
    ui::success(format!(
        "Themes dir  →  {} installed  ({})",
        themes_count,
        config::themes_dir().display()
    ));

    ui::blank();
}

// ── Update ────────────────────────────────────────────────────────────────────

pub fn update(channel_override: Option<String>) {
    updater::run_update_command(channel_override);
}

// ── Shared helpers ────────────────────────────────────────────────────────────

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
    let mut archive = zip::ZipArchive::new(cursor)
        .unwrap_or_else(|e| ui::die(format!("invalid bundle for {name}: {e}")));

    let nxb = {
        let mut e = archive
            .by_name("app.nxb")
            .unwrap_or_else(|_| ui::die("bundle missing app.nxb"));
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).unwrap();
        buf
    };
    let manifest_str = {
        let mut e = archive
            .by_name("manifest.json")
            .unwrap_or_else(|_| ui::die("bundle missing manifest.json"));
        let mut buf = String::new();
        e.read_to_string(&mut buf).unwrap();
        buf
    };
    let sig_str = {
        let mut e = archive
            .by_name("signature.sig")
            .unwrap_or_else(|_| ui::die("bundle missing signature.sig"));
        let mut buf = String::new();
        e.read_to_string(&mut buf).unwrap();
        buf
    };

    let mut hasher = Sha256::new();
    hasher.update(&nxb);
    hasher.update(manifest_str.as_bytes());
    let computed = format!("{:x}", hasher.finalize());
    if computed != sig_str.trim() {
        ui::die(format!(
            "signature verification failed for {name} — bundle may be corrupted"
        ));
    }
}

fn extract_bundle_to(bundle: &[u8], dest: &Path) {
    use std::io::{Cursor, Read as _};
    fs::create_dir_all(dest)
        .unwrap_or_else(|e| ui::die(format!("cannot create {}: {e}", dest.display())));
    let cursor = Cursor::new(bundle);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let out_path = dest.join(entry.name());
        // Create parent directories for entries like src/app.nx
        if let Some(parent) = out_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        fs::write(&out_path, &buf)
            .unwrap_or_else(|e| ui::die(format!("write {}: {e}", out_path.display())));
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
    fs::read_to_string(libs_dir.join(".lock"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_lockfile(libs_dir: &Path, lock: &Lockfile) {
    let json = serde_json::to_string_pretty(lock).expect("serialize lockfile");
    fs::write(libs_dir.join(".lock"), json).unwrap_or_else(|e| {
        ui::warn(format!("could not write lockfile: {e}"));
    });
}

/// Write installed packages back into `project.json` dependencies.
///
/// Reads the existing `project.json`, merges the newly resolved versions into
/// `"dependencies"`, then writes it back. Uses the version from the lockfile
/// so that `"latest"` requests are pinned to the actual installed version.
fn update_project_dependencies(
    project_root: &Path,
    installed: &[(String, String)],
    lock: &Lockfile,
) {
    let path = project_root.join("project.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };

    if let Some(obj) = value.as_object_mut() {
        let deps = obj
            .entry("dependencies")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(deps_map) = deps.as_object_mut() {
            for (name, _requested_ver) in installed {
                // Pin to the version actually recorded in the lockfile
                // ("latest" input → real resolved version like "1.0.0")
                let pinned = lock
                    .packages
                    .iter()
                    .find(|e| &e.name == name)
                    .map(|e| e.version.as_str())
                    .unwrap_or("latest");
                deps_map.insert(name.clone(), serde_json::Value::String(pinned.to_string()));
            }
        }
    }

    if let Ok(updated) = serde_json::to_string_pretty(&value) {
        let _ = fs::write(&path, updated);
    }
}

// ── Build helper ──────────────────────────────────────────────────────────────

pub fn write_dist(dist_dir: &Path, result: nexa_compiler::CompileResult) {
    fs::create_dir_all(dist_dir).expect("cannot create dist/");
    fs::write(dist_dir.join("index.html"), &result.html).expect("cannot write index.html");
    fs::write(dist_dir.join("app.js"), &result.js).expect("cannot write app.js");
}
