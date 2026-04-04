use super::{init::to_pascal_case, init::write_file, load_project};
use crate::infrastructure::ui;
use std::{fs, path::PathBuf};

pub fn module_add(name: String, project_dir: Option<PathBuf>) {
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        ui::die(format!(
            "module name '{}' must only contain letters, digits, hyphens or underscores",
            name
        ));
    }

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

    let module_json = format!(
        r#"{{
  "name": "{name}",
  "main": "app.nx",
  "dependencies": {{}}
}}
"#
    );
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
    if let Ok(text) = fs::read_to_string(&proj_path) {
        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(modules) = val.get_mut("modules").and_then(|m| m.as_array_mut()) {
                modules.push(serde_json::Value::String(name.clone()));
            }
            if let Ok(updated) = serde_json::to_string_pretty(&val) {
                let _ = fs::write(&proj_path, updated);
            }
        }
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

    #[test]
    fn module_add_creates_module_directory_structure() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add("api".to_string(), Some(tmp.path().to_path_buf()));

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

        module_add("api".to_string(), Some(tmp.path().to_path_buf()));

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("api").join("module.json"),
        )
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

        module_add("my-service".to_string(), Some(tmp.path().to_path_buf()));

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
        assert!(raw.contains("package my_service"), "expected 'package my_service' in:\n{raw}");
    }

    #[test]
    fn module_add_updates_project_json_modules_list() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add("api".to_string(), Some(tmp.path().to_path_buf()));

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

        module_add("api".to_string(), Some(tmp.path().to_path_buf()));
        module_add("worker".to_string(), Some(tmp.path().to_path_buf()));

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
}
