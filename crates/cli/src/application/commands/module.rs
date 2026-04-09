use super::{init::to_pascal_case, init::write_file, load_project};
use crate::application::project::{AppType, Platform};
use crate::infrastructure::ui;
use serde::Serialize;
use std::{fs, path::PathBuf};

// ── Parsing helpers ───────────────────────────────────────────────────────────

/// Parse a kebab-case string into an [`AppType`].
/// Unknown strings fall back to [`AppType::Web`].
pub fn parse_app_type(s: &str) -> AppType {
    match s {
        "backend" => AppType::Backend,
        "cli" => AppType::Cli,
        "desktop" => AppType::Desktop,
        "package" => AppType::Package,
        _ => AppType::Web,
    }
}

/// Parse a single kebab-case platform string into a [`Platform`].
/// Returns `None` for unrecognised values.
pub fn parse_platform(s: &str) -> Option<Platform> {
    match s {
        "browser" => Some(Platform::Browser),
        "native" => Some(Platform::Native),
        "native-linux" => Some(Platform::NativeLinux),
        "native-macos" => Some(Platform::NativeMacos),
        "native-windows" => Some(Platform::NativeWindows),
        "macos" => Some(Platform::Macos),
        "windows" => Some(Platform::Windows),
        "linux" => Some(Platform::Linux),
        "ios" => Some(Platform::Ios),
        "android" => Some(Platform::Android),
        _ => None,
    }
}

// ── Command ───────────────────────────────────────────────────────────────────

pub fn module_add(
    name: String,
    project_dir: Option<PathBuf>,
    module_type: String,
    platforms_raw: Option<String>,
) {
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        ui::die(format!(
            "module name '{}' must only contain letters, digits, hyphens or underscores",
            name
        ));
    }

    let app_type = parse_app_type(&module_type);

    // Parse comma-separated platforms, silently ignoring unknown values.
    let platforms: Vec<Platform> = platforms_raw
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .filter_map(parse_platform)
                .collect()
        })
        .unwrap_or_default();

    let proj = load_project(project_dir);

    if proj.project.modules.contains(&name) {
        ui::die(format!("module '{name}' already exists in this project."));
    }

    let root = proj.root().to_path_buf();
    let src_main = root.join("modules").join(&name).join("src").join("main");
    let src_test = root.join("modules").join(&name).join("src").join("test");

    fs::create_dir_all(&src_main)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory: {e}")));
    fs::create_dir_all(&src_test)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory: {e}")));

    // Build module.json — only include optional fields when non-default.
    let module_json = build_module_json(&name, &app_type, &platforms);
    write_file(
        &root.join("modules").join(&name).join("module.json"),
        &module_json,
    );

    let app_class = to_pascal_case(&name);
    let app_nx = format!(
        r#"package {pkg};

app {app} {{
  server {{ port: 3000; }}

  public window HomePage {{
    public render() => Component {{
      return Page {{
        Heading("Module {app}")
      }};
    }}
  }}

  route "/" => HomePage;
}}
"#,
        pkg = name.replace('-', "_"),
        app = app_class,
    );
    write_file(&src_main.join("app.nx"), &app_nx);

    // Add module to project.json
    let proj_path = root.join("project.json");
    match fs::read_to_string(&proj_path) {
        Err(e) => ui::warn(format!("could not read project.json: {e}")),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Err(e) => ui::warn(format!("could not parse project.json: {e}")),
            Ok(mut val) => {
                if let Some(modules) = val.get_mut("modules").and_then(|m| m.as_array_mut()) {
                    modules.push(serde_json::Value::String(name.clone()));
                } else {
                    ui::warn("project.json has no 'modules' array — module not registered");
                }
                match serde_json::to_string_pretty(&val) {
                    Err(e) => ui::warn(format!("could not serialize project.json: {e}")),
                    Ok(updated) => {
                        if let Err(e) = fs::write(&proj_path, updated) {
                            ui::warn(format!("could not write project.json: {e}"));
                        }
                    }
                }
            }
        },
    }

    ui::blank();
    ui::success(format!("Module \x1b[1m{name}\x1b[0m added"));
    ui::blank();
    ui::hint(format!("  modules/{name}/"));
    ui::hint("  ├── module.json");
    ui::hint("  └── src/main/app.nx");
    ui::blank();
    ui::hint(format!(
        "  Set as main:  nexa-compiler.yaml → main_module: \"{name}\""
    ));
    ui::blank();
}

/// Construct the `module.json` content.
///
/// The `"type"` field is omitted when the type is `Web` (the default) and
/// `"platforms"` is omitted when the list is empty — both for backward
/// compatibility with projects that do not use multi-target features.
fn build_module_json(name: &str, app_type: &AppType, platforms: &[Platform]) -> String {
    #[derive(Serialize)]
    struct ModuleJson<'a> {
        name: &'a str,
        main: &'static str,
        #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
        app_type: Option<&'static str>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        platforms: Vec<&'static str>,
        dependencies: serde_json::Map<String, serde_json::Value>,
    }

    let type_str = match app_type {
        AppType::Web => None,
        AppType::Backend => Some("backend"),
        AppType::Cli => Some("cli"),
        AppType::Desktop => Some("desktop"),
        AppType::Package => Some("package"),
    };

    let data = ModuleJson {
        name,
        main: "app.nx",
        app_type: type_str,
        platforms: platforms.iter().map(|p| p.as_str()).collect(),
        dependencies: serde_json::Map::new(),
    };

    let mut out = serde_json::to_string_pretty(&data)
        .expect("ModuleJson serialization is infallible");
    out.push('\n');
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};
    use tempfile::TempDir;

    /// Minimal valid project layout that `load_project` accepts.
    fn make_project(dir: &Path) {
        fs::write(
            dir.join("project.json"),
            r#"{"name":"test","version":"0.1.0","author":"Dev","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("nexa-compiler.yaml"),
            "version: \"0.1\"\nmain_module: \"core\"\n",
        )
        .unwrap();
        let src = dir.join("modules").join("core").join("src").join("main");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            dir.join("modules").join("core").join("module.json"),
            r#"{"name":"core","main":"app.nx"}"#,
        )
        .unwrap();
        fs::write(src.join("app.nx"), "").unwrap();
    }

    // ── parse_app_type ────────────────────────────────────────────────────────

    #[test]
    fn parse_app_type_known_variants() {
        assert_eq!(parse_app_type("backend"), AppType::Backend);
        assert_eq!(parse_app_type("cli"), AppType::Cli);
        assert_eq!(parse_app_type("desktop"), AppType::Desktop);
        assert_eq!(parse_app_type("package"), AppType::Package);
        assert_eq!(parse_app_type("web"), AppType::Web);
    }

    #[test]
    fn parse_app_type_unknown_falls_back_to_web() {
        assert_eq!(parse_app_type("unknown"), AppType::Web);
        assert_eq!(parse_app_type(""), AppType::Web);
    }

    // ── parse_platform ────────────────────────────────────────────────────────

    #[test]
    fn parse_platform_all_variants() {
        assert_eq!(parse_platform("browser"), Some(Platform::Browser));
        assert_eq!(parse_platform("native"), Some(Platform::Native));
        assert_eq!(parse_platform("native-linux"), Some(Platform::NativeLinux));
        assert_eq!(parse_platform("native-macos"), Some(Platform::NativeMacos));
        assert_eq!(parse_platform("native-windows"), Some(Platform::NativeWindows));
        assert_eq!(parse_platform("macos"), Some(Platform::Macos));
        assert_eq!(parse_platform("windows"), Some(Platform::Windows));
        assert_eq!(parse_platform("linux"), Some(Platform::Linux));
        assert_eq!(parse_platform("ios"), Some(Platform::Ios));
        assert_eq!(parse_platform("android"), Some(Platform::Android));
    }

    #[test]
    fn parse_platform_unknown_returns_none() {
        assert_eq!(parse_platform("unknown"), None);
        assert_eq!(parse_platform(""), None);
    }

    // ── build_module_json ─────────────────────────────────────────────────────

    #[test]
    fn build_module_json_web_omits_type_and_platforms() {
        let json = build_module_json("my-web", &AppType::Web, &[]);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["name"].as_str(), Some("my-web"));
        assert!(val.get("type").is_none(), "type should be absent for web");
        assert!(
            val.get("platforms").is_none(),
            "platforms should be absent when empty"
        );
    }

    #[test]
    fn build_module_json_backend_includes_type() {
        let json = build_module_json("my-api", &AppType::Backend, &[]);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["type"].as_str(), Some("backend"));
        assert!(val.get("platforms").is_none());
    }

    #[test]
    fn build_module_json_cli_with_platforms() {
        let json =
            build_module_json("my-cli", &AppType::Cli, &[Platform::Native]);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["type"].as_str(), Some("cli"));
        let plats: Vec<&str> = val["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(plats, vec!["native"]);
    }

    // ── module_add (integration) ──────────────────────────────────────────────

    #[test]
    fn module_add_creates_module_directory_structure() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );

        assert!(tmp.path().join("modules").join("api").join("module.json").exists());
        assert!(tmp
            .path()
            .join("modules")
            .join("api")
            .join("src")
            .join("main")
            .join("app.nx")
            .exists());
        assert!(tmp
            .path()
            .join("modules")
            .join("api")
            .join("src")
            .join("test")
            .is_dir());
    }

    #[test]
    fn module_add_module_json_has_correct_name_and_main() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );

        let raw =
            fs::read_to_string(tmp.path().join("modules").join("api").join("module.json"))
                .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["name"].as_str(), Some("api"));
        assert_eq!(val["main"].as_str(), Some("app.nx"));
        assert!(val["dependencies"].as_object().unwrap().is_empty());
    }

    #[test]
    fn module_add_app_nx_uses_pascal_case() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "my-service".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );

        let raw = fs::read_to_string(
            tmp.path()
                .join("modules")
                .join("my-service")
                .join("src")
                .join("main")
                .join("app.nx"),
        )
        .unwrap();
        assert!(raw.contains("app MyService"), "expected 'app MyService' in:\n{raw}");
        assert!(
            raw.contains("package my_service"),
            "expected 'package my_service' in:\n{raw}"
        );
    }

    #[test]
    fn module_add_updates_project_json_modules_list() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );

        let raw = fs::read_to_string(tmp.path().join("project.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let modules: Vec<_> = val["modules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m.as_str())
            .collect();
        assert!(modules.contains(&"core"), "core should still be present");
        assert!(modules.contains(&"api"), "api should be added");
    }

    #[test]
    fn module_add_multiple_modules_accumulate() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );
        module_add(
            "worker".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(),
            None,
        );

        let raw = fs::read_to_string(tmp.path().join("project.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let modules: Vec<_> = val["modules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m.as_str())
            .collect();
        assert_eq!(modules.len(), 3); // core + api + worker
        assert!(modules.contains(&"worker"));
    }

    // ── Task 6 new tests ──────────────────────────────────────────────────────

    /// `nexa module add my-api --type backend` → module.json contains "type": "backend"
    #[test]
    fn module_add_type_backend_writes_type_field() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "my-api".to_string(),
            Some(tmp.path().to_path_buf()),
            "backend".to_string(),
            None,
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("my-api").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            val["type"].as_str(),
            Some("backend"),
            "module.json should contain \"type\": \"backend\""
        );
        assert!(
            val.get("platforms").is_none(),
            "platforms should be absent when not specified"
        );
    }

    /// `nexa module add my-web` (no --type) → module.json has no "type" field (backward compat)
    #[test]
    fn module_add_default_web_omits_type_field() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "my-web".to_string(),
            Some(tmp.path().to_path_buf()),
            "web".to_string(), // default supplied by clap
            None,
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("my-web").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            val.get("type").is_none(),
            "type field must be absent for web modules (backward compat)"
        );
        assert!(
            val.get("platforms").is_none(),
            "platforms field must be absent when not specified"
        );
    }

    /// `nexa module add my-cli --type cli --platforms native` →
    /// module.json has both "type": "cli" and "platforms": ["native"]
    #[test]
    fn module_add_cli_with_platforms_writes_both_fields() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "my-cli".to_string(),
            Some(tmp.path().to_path_buf()),
            "cli".to_string(),
            Some("native".to_string()),
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("my-cli").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("cli"));
        let plats: Vec<&str> = val["platforms"]
            .as_array()
            .expect("platforms should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(plats, vec!["native"]);
    }

    /// Multiple platforms separated by commas.
    #[test]
    fn module_add_multiple_platforms_parsed_correctly() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "cross".to_string(),
            Some(tmp.path().to_path_buf()),
            "backend".to_string(),
            Some("native-linux,native-macos".to_string()),
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("cross").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let plats: Vec<&str> = val["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(plats, vec!["native-linux", "native-macos"]);
    }

    /// Unknown platforms are silently dropped.
    #[test]
    fn module_add_unknown_platform_is_ignored() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "srv".to_string(),
            Some(tmp.path().to_path_buf()),
            "backend".to_string(),
            Some("native,bogus-platform".to_string()),
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("srv").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let plats: Vec<&str> = val["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // Only "native" survives; "bogus-platform" is silently dropped.
        assert_eq!(plats, vec!["native"]);
    }

    /// Desktop module type.
    #[test]
    fn module_add_desktop_type() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "gui".to_string(),
            Some(tmp.path().to_path_buf()),
            "desktop".to_string(),
            None,
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("gui").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("desktop"));
    }

    /// Package module type.
    #[test]
    fn module_add_package_type() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "my-lib".to_string(),
            Some(tmp.path().to_path_buf()),
            "package".to_string(),
            None,
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("my-lib").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("package"));
    }
}
