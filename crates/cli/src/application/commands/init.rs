use crate::infrastructure::ui;
use std::{fs, path::Path, path::PathBuf};

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

    create_project_files(&root, &project_name, &author_str, &version);

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
    ui::hint("  └── modules/core/");
    ui::hint("      ├── module.json");
    ui::hint("      └── src/main/app.nx");
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
    println!("  \x1b[1mnexa run\x1b[0m                    start the dev server");
    println!("  \x1b[1mnexa build\x1b[0m                  compile the main module");
    println!("  \x1b[1mnexa module add <name>\x1b[0m      add a new module");
    ui::blank();
}

/// Create all project files and directories under `root`.
/// Extracted for unit-testability — `init()` delegates here after resolving paths/author.
fn create_project_files(root: &Path, project_name: &str, author: &str, version: &str) {
    let core_src_main = root.join("modules").join("core").join("src").join("main");
    let core_src_test = root.join("modules").join("core").join("src").join("test");
    fs::create_dir_all(&core_src_main)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory structure: {e}")));
    fs::create_dir_all(&core_src_test)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory structure: {e}")));

    let project_json = format!(
        r#"{{
  "name": "{name}",
  "version": "{ver}",
  "author": "{author}",
  "modules": ["core"],
  "dependencies": {{}}
}}
"#,
        name = project_name,
        ver = version,
        author = author,
    );
    write_file(&root.join("project.json"), &project_json);

    let module_json = r#"{
  "name": "core",
  "main": "app.nx",
  "dependencies": {}
}
"#;
    write_file(
        &root.join("modules").join("core").join("module.json"),
        module_json,
    );

    let compiler_yaml = r#"version: "0.1"
main_module: "core"
# include_modules:
#   - core
# exclude_modules: []
# registry: "https://registry.nexa-lang.org"
# private_registries:
#   - url: "https://corp.registry.example.com"
#     key: "sk_live_..."
"#;
    write_file(&root.join("nexa-compiler.yaml"), compiler_yaml);

    let app_class = to_pascal_case(project_name);
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
    write_file(&core_src_main.join("app.nx"), &app_nx);

    let gitignore = r#"# Nexa compiler output
dist/
.nexa/

# Installed packages (project-level and module-level)
lib/
modules/*/lib/

# Distributable bundles
*.nexa

# macOS
.DS_Store

# Environment variables
.env
"#;
    write_file(&root.join(".gitignore"), gitignore);
}

pub(super) fn write_file(path: &Path, content: &str) {
    fs::write(path, content)
        .unwrap_or_else(|e| ui::die(format!("cannot write {}: {e}", path.display())));
}

pub(super) fn to_pascal_case(s: &str) -> String {
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
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(to_pascal_case("my-app"), "MyApp");
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
        assert_eq!(to_pascal_case("my-cool-app"), "MyCoolApp");
        assert_eq!(to_pascal_case("app"), "App");
        assert_eq!(to_pascal_case("a-b-c"), "ABC");
        assert_eq!(to_pascal_case(""), "");
    }

    // ── create_project_files tests ────────────────────────────────────────────

    #[test]
    fn creates_expected_directory_structure() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "my-app", "Dev", "0.1.0");

        assert!(tmp.path().join("project.json").exists());
        assert!(tmp.path().join("nexa-compiler.yaml").exists());
        assert!(tmp.path().join(".gitignore").exists());
        assert!(tmp
            .path()
            .join("modules")
            .join("core")
            .join("module.json")
            .exists());
        assert!(tmp
            .path()
            .join("modules")
            .join("core")
            .join("src")
            .join("main")
            .join("app.nx")
            .exists());
        assert!(tmp
            .path()
            .join("modules")
            .join("core")
            .join("src")
            .join("test")
            .is_dir());
    }

    #[test]
    fn project_json_has_correct_content() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "my-app", "Alice", "1.2.3");

        let raw = fs::read_to_string(tmp.path().join("project.json")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["name"].as_str(), Some("my-app"));
        assert_eq!(val["version"].as_str(), Some("1.2.3"));
        assert_eq!(val["author"].as_str(), Some("Alice"));
        let modules = val["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].as_str(), Some("core"));
        assert!(val["dependencies"].as_object().unwrap().is_empty());
    }

    #[test]
    fn module_json_has_correct_content() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "proj", "Dev", "0.1.0");

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("core").join("module.json"),
        )
        .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["name"].as_str(), Some("core"));
        assert_eq!(val["main"].as_str(), Some("app.nx"));
    }

    #[test]
    fn compiler_yaml_has_correct_content() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "proj", "Dev", "0.1.0");

        let raw = fs::read_to_string(tmp.path().join("nexa-compiler.yaml")).unwrap();
        assert!(raw.contains("main_module: \"core\""));
        assert!(raw.contains("version: \"0.1\""));
    }

    #[test]
    fn app_nx_uses_pascal_case_class_name() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "my-cool-app", "Dev", "0.1.0");

        let raw = fs::read_to_string(
            tmp.path()
                .join("modules")
                .join("core")
                .join("src")
                .join("main")
                .join("app.nx"),
        )
        .unwrap();
        assert!(raw.contains("app MyCoolApp"), "expected 'app MyCoolApp' in:\n{raw}");
        assert!(raw.contains("package my_cool_app"), "expected 'package my_cool_app' in:\n{raw}");
    }

    #[test]
    fn gitignore_excludes_build_artifacts() {
        let tmp = TempDir::new().unwrap();
        create_project_files(tmp.path(), "proj", "Dev", "0.1.0");

        let raw = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(raw.contains("lib/"));
        assert!(raw.contains("modules/*/lib/"));
        assert!(raw.contains("*.nexa"));
        assert!(raw.contains(".env"));
    }
}
