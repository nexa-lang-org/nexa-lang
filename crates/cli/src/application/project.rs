use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use thiserror::Error;

// ── Errors ────────────────────────────────────────────────────────────────────

/// Errors that can occur while loading or validating a Nexa project.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("project.json not found in '{0}' — are you inside a Nexa project?")]
    MissingProjectJson(PathBuf),

    #[error("nexa-compiler.yaml not found in '{0}'")]
    MissingCompilerYaml(PathBuf),

    #[error("module.json not found for module '{0}' (expected: {1})")]
    MissingModuleJson(String, PathBuf),

    #[error("src/main/ directory not found for module '{0}' (expected: {1})")]
    MissingModuleSrcMain(String, PathBuf),

    #[error("entry file not found for module '{0}': '{1}'")]
    MissingModuleEntryFile(String, PathBuf),

    #[error("no modules defined — add at least one module to project.json")]
    NoModules,

    #[error("main_module '{0}' is not listed in project.json modules")]
    UnknownMainModule(String),

    #[error("failed to read project.json: {0}")]
    ReadProjectJson(#[source] std::io::Error),

    #[error("failed to parse project.json: {0}")]
    ParseProjectJson(#[source] serde_json::Error),

    #[error("failed to read nexa-compiler.yaml: {0}")]
    ReadCompilerYaml(#[source] std::io::Error),

    #[error("failed to parse nexa-compiler.yaml: {0}")]
    ParseCompilerYaml(#[source] serde_yaml::Error),

    #[error("failed to read module.json for '{0}': {1}")]
    ReadModuleJson(String, #[source] std::io::Error),

    #[error("failed to parse module.json for '{0}': {1}")]
    ParseModuleJson(String, #[source] serde_json::Error),
}

// ── Config structs ────────────────────────────────────────────────────────────

/// Deserialized from `project.json` at the project root.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    pub version: String,
    pub author: String,
    /// List of module names (subdirectories of `modules/`).
    pub modules: Vec<String>,
    /// Project-level package dependencies shared across all modules.
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

/// Deserialized from `modules/<name>/module.json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModuleConfig {
    pub name: String,
    /// Entry file name inside `src/main/`, e.g. `"app.nx"`.
    pub main: String,
    /// Module-specific dependencies (installed in `modules/<name>/lib/`).
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

/// A private registry declared in `nexa-compiler.yaml`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PrivateRegistry {
    /// Registry base URL.
    pub url: String,
    /// API key used for authenticated requests.
    pub key: String,
}

fn default_main_module() -> String {
    "core".to_string()
}

/// Deserialized from `nexa-compiler.yaml` at the project root.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CompilerConfig {
    pub version: String,
    /// The module to use as the compilation entry point (default: `"core"`).
    #[serde(default = "default_main_module")]
    pub main_module: String,
    /// If set, only these modules are compiled. Overrides `exclude_modules`.
    #[serde(default)]
    pub include_modules: Option<Vec<String>>,
    /// Modules to skip during compilation (ignored if `include_modules` is set).
    #[serde(default)]
    pub exclude_modules: Vec<String>,
    /// Public registry URL (default: `https://registry.nexa-lang.org`).
    #[serde(default)]
    pub registry: Option<String>,
    /// Private registries with API keys, tried before the public registry.
    #[serde(default)]
    pub private_registries: Vec<PrivateRegistry>,
}

impl CompilerConfig {
    pub const DEFAULT_REGISTRY: &'static str = "https://registry.nexa-lang.org";

    /// Returns all registries to try in order: private first, then public.
    pub fn all_registries(&self) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = self
            .private_registries
            .iter()
            .map(|r| (r.url.clone(), Some(r.key.clone())))
            .collect();
        let public = self
            .registry
            .clone()
            .unwrap_or_else(|| Self::DEFAULT_REGISTRY.to_string());
        out.push((public, None));
        out
    }

    /// Returns the list of modules that should be compiled, applying
    /// `include_modules` / `exclude_modules` filters.
    pub fn active_modules<'a>(&'a self, all_modules: &'a [String]) -> Vec<&'a str> {
        if let Some(include) = &self.include_modules {
            return include.iter().map(String::as_str).collect();
        }
        all_modules
            .iter()
            .filter(|m| !self.exclude_modules.contains(m))
            .map(String::as_str)
            .collect()
    }
}

// ── Project ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NexaProject {
    root: PathBuf,
    pub project: ProjectConfig,
    pub compiler: CompilerConfig,
    /// Loaded module configs keyed by module name.
    pub modules: HashMap<String, ModuleConfig>,
}

// ── Pure parsing functions (independently testable) ──────────────────────────

/// Parse `project.json` content into a [`ProjectConfig`].
pub fn parse_project_config(text: &str) -> Result<ProjectConfig, ProjectError> {
    serde_json::from_str(text).map_err(ProjectError::ParseProjectJson)
}

/// Parse `nexa-compiler.yaml` content into a [`CompilerConfig`].
pub fn parse_compiler_config(text: &str) -> Result<CompilerConfig, ProjectError> {
    serde_yaml::from_str(text).map_err(ProjectError::ParseCompilerYaml)
}

/// Parse `module.json` content into a [`ModuleConfig`].
pub fn parse_module_config(text: &str, name: &str) -> Result<ModuleConfig, ProjectError> {
    serde_json::from_str(text).map_err(|e| ProjectError::ParseModuleJson(name.to_string(), e))
}

// ── Implementation ────────────────────────────────────────────────────────────

impl NexaProject {
    /// Load and validate a Nexa project from `dir`.
    ///
    /// Reads `project.json`, `nexa-compiler.yaml`, and all `modules/<name>/module.json`
    /// files, then verifies the main module's entry file exists.
    pub fn load(dir: &Path) -> Result<Self, ProjectError> {
        let root = dir.to_path_buf();

        let project = fs::read_to_string(root.join("project.json"))
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => ProjectError::MissingProjectJson(root.clone()),
                _ => ProjectError::ReadProjectJson(e),
            })
            .and_then(|t| parse_project_config(&t))?;

        let compiler = fs::read_to_string(root.join("nexa-compiler.yaml"))
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => ProjectError::MissingCompilerYaml(root.clone()),
                _ => ProjectError::ReadCompilerYaml(e),
            })
            .and_then(|t| parse_compiler_config(&t))?;

        if project.modules.is_empty() {
            return Err(ProjectError::NoModules);
        }

        if !project.modules.contains(&compiler.main_module) {
            return Err(ProjectError::UnknownMainModule(
                compiler.main_module.clone(),
            ));
        }

        // Load each module's module.json
        let mut modules = HashMap::new();
        for mod_name in &project.modules {
            let module_json_path = root.join("modules").join(mod_name).join("module.json");

            let module_cfg = fs::read_to_string(&module_json_path)
                .map_err(|e| match e.kind() {
                    ErrorKind::NotFound => {
                        ProjectError::MissingModuleJson(mod_name.clone(), module_json_path.clone())
                    }
                    _ => ProjectError::ReadModuleJson(mod_name.clone(), e),
                })
                .and_then(|t| parse_module_config(&t, mod_name))?;

            // Validate src/main/ exists for this module
            let src_main = root.join("modules").join(mod_name).join("src").join("main");
            if !src_main.is_dir() {
                return Err(ProjectError::MissingModuleSrcMain(
                    mod_name.clone(),
                    src_main,
                ));
            }

            // Validate entry file exists
            let entry = src_main.join(&module_cfg.main);
            if !entry.exists() {
                return Err(ProjectError::MissingModuleEntryFile(
                    mod_name.clone(),
                    entry,
                ));
            }

            modules.insert(mod_name.clone(), module_cfg);
        }

        let proj = NexaProject {
            root,
            project,
            compiler,
            modules,
        };
        proj.ensure_optional_dirs();
        Ok(proj)
    }

    /// Silently create optional directories that should exist for each module.
    fn ensure_optional_dirs(&self) {
        for mod_name in &self.project.modules {
            let _ = fs::create_dir_all(self.module_test_dir(mod_name));
        }
    }

    // ── Root paths ────────────────────────────────────────────────────────────

    /// Returns the project root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns `<root>/modules/`.
    pub fn modules_dir(&self) -> PathBuf {
        self.root.join("modules")
    }

    /// Returns `<root>/lib/` — project-level installed packages.
    pub fn lib_dir(&self) -> PathBuf {
        self.root.join("lib")
    }

    // ── Module paths ──────────────────────────────────────────────────────────

    /// Returns `<root>/modules/<name>/`.
    pub fn module_dir(&self, name: &str) -> PathBuf {
        self.root.join("modules").join(name)
    }

    /// Returns `<root>/modules/<name>/src/main/` — the compiler src_root for a module.
    pub fn module_src_root(&self, name: &str) -> PathBuf {
        self.module_dir(name).join("src").join("main")
    }

    /// Returns the entry file for a module: `<root>/modules/<name>/src/main/<module.main>`.
    pub fn module_entry(&self, name: &str) -> PathBuf {
        let main = self
            .modules
            .get(name)
            .map(|m| m.main.as_str())
            .unwrap_or("app.nx");
        self.module_src_root(name).join(main)
    }

    /// Returns `<root>/modules/<name>/src/test/`.
    pub fn module_test_dir(&self, name: &str) -> PathBuf {
        self.module_dir(name).join("src").join("test")
    }

    /// Returns `<root>/modules/<name>/lib/` — module-specific installed packages.
    pub fn module_lib_dir(&self, name: &str) -> PathBuf {
        self.module_dir(name).join("lib")
    }

    /// Returns `<root>/modules/<name>/src/dist/` — build output for a module.
    pub fn dist_dir(&self, name: &str) -> PathBuf {
        self.module_dir(name).join("src").join("dist")
    }

    // ── Main module shortcuts ─────────────────────────────────────────────────

    /// Returns the name of the main (entry) module as defined in `nexa-compiler.yaml`.
    pub fn main_module_name(&self) -> &str {
        &self.compiler.main_module
    }

    /// Returns the entry file of the main module.
    pub fn main_entry(&self) -> PathBuf {
        self.module_entry(self.main_module_name())
    }

    /// Returns `src/main/` of the main module (the compiler's `src_root`).
    pub fn main_src_root(&self) -> PathBuf {
        self.module_src_root(self.main_module_name())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal valid new-style project layout in `dir`.
    fn make_valid_project(dir: &Path) {
        fs::write(
            dir.join("project.json"),
            r#"{"name":"test-app","version":"0.1.0","author":"Tester","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("nexa-compiler.yaml"),
            "version: \"0.1\"\nmain_module: \"core\"\n",
        )
        .unwrap();
        let src_main = dir.join("modules").join("core").join("src").join("main");
        fs::create_dir_all(&src_main).unwrap();
        fs::write(
            dir.join("modules").join("core").join("module.json"),
            r#"{"name":"core","main":"app.nx"}"#,
        )
        .unwrap();
        fs::write(src_main.join("app.nx"), "").unwrap();
    }

    #[test]
    fn parse_project_config_valid() {
        let json = r#"{"name":"my-app","version":"1.0.0","author":"Dev","modules":["core"]}"#;
        let cfg = parse_project_config(json).unwrap();
        assert_eq!(cfg.name, "my-app");
        assert_eq!(cfg.modules, vec!["core"]);
        assert!(cfg.dependencies.is_empty());
    }

    #[test]
    fn parse_project_config_dependencies_optional() {
        let json = r#"{"name":"a","version":"1","author":"b","modules":["core"]}"#;
        let cfg = parse_project_config(json).unwrap();
        assert!(cfg.dependencies.is_empty());
    }

    #[test]
    fn parse_project_config_with_dependencies() {
        let json = r#"{"name":"a","version":"1","author":"b","modules":["core"],"dependencies":{"my-lib":"^1.0.0"}}"#;
        let cfg = parse_project_config(json).unwrap();
        assert_eq!(
            cfg.dependencies.get("my-lib").map(String::as_str),
            Some("^1.0.0")
        );
    }

    #[test]
    fn parse_project_config_missing_required_field() {
        let json = r#"{"name":"a","version":"1","author":"b"}"#;
        assert!(parse_project_config(json).is_err());
    }

    #[test]
    fn parse_module_config_valid() {
        let json = r#"{"name":"core","main":"app.nx"}"#;
        let cfg = parse_module_config(json, "core").unwrap();
        assert_eq!(cfg.name, "core");
        assert_eq!(cfg.main, "app.nx");
        assert!(cfg.dependencies.is_empty());
    }

    #[test]
    fn parse_compiler_config_defaults() {
        let cfg = parse_compiler_config("version: \"0.1\"\n").unwrap();
        assert_eq!(cfg.version, "0.1");
        assert_eq!(cfg.main_module, "core");
        assert!(cfg.registry.is_none());
        assert!(cfg.private_registries.is_empty());
    }

    #[test]
    fn parse_compiler_config_with_main_module() {
        let yaml = "version: \"0.1\"\nmain_module: \"api\"\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        assert_eq!(cfg.main_module, "api");
    }

    #[test]
    fn parse_compiler_config_with_include_modules() {
        let yaml = "version: \"0.1\"\ninclude_modules:\n  - core\n  - api\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        assert_eq!(cfg.include_modules, Some(vec!["core".into(), "api".into()]));
    }

    #[test]
    fn active_modules_all_by_default() {
        let cfg = parse_compiler_config("version: \"0.1\"\n").unwrap();
        let all = vec!["core".to_string(), "api".to_string()];
        assert_eq!(cfg.active_modules(&all), vec!["core", "api"]);
    }

    #[test]
    fn active_modules_with_exclude() {
        let yaml = "version: \"0.1\"\nexclude_modules:\n  - api\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        let all = vec!["core".to_string(), "api".to_string()];
        assert_eq!(cfg.active_modules(&all), vec!["core"]);
    }

    #[test]
    fn active_modules_include_overrides_exclude() {
        let yaml = "version: \"0.1\"\ninclude_modules:\n  - core\nexclude_modules:\n  - core\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        let all = vec!["core".to_string(), "api".to_string()];
        assert_eq!(cfg.active_modules(&all), vec!["core"]);
    }

    #[test]
    fn compiler_config_all_registries_order() {
        let yaml = "version: \"0.1\"\nregistry: \"https://pub.reg\"\nprivate_registries:\n  - url: \"https://priv.reg\"\n    key: \"k\"\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        let regs = cfg.all_registries();
        assert_eq!(regs[0].0, "https://priv.reg");
        assert_eq!(regs[1].0, "https://pub.reg");
    }

    #[test]
    fn path_helpers_are_correct() {
        let tmp = TempDir::new().unwrap();
        make_valid_project(tmp.path());
        let proj = NexaProject::load(tmp.path()).unwrap();

        assert_eq!(proj.modules_dir(), tmp.path().join("modules"));
        assert_eq!(proj.lib_dir(), tmp.path().join("lib"));
        assert_eq!(
            proj.module_src_root("core"),
            tmp.path()
                .join("modules")
                .join("core")
                .join("src")
                .join("main")
        );
        assert_eq!(
            proj.module_entry("core"),
            tmp.path()
                .join("modules")
                .join("core")
                .join("src")
                .join("main")
                .join("app.nx")
        );
        assert_eq!(
            proj.dist_dir("core"),
            tmp.path()
                .join("modules")
                .join("core")
                .join("src")
                .join("dist")
        );
        assert_eq!(proj.main_module_name(), "core");
        assert_eq!(proj.main_entry(), proj.module_entry("core"));
    }

    #[test]
    fn load_valid_project_succeeds() {
        let tmp = TempDir::new().unwrap();
        make_valid_project(tmp.path());
        assert!(NexaProject::load(tmp.path()).is_ok());
    }

    #[test]
    fn load_creates_optional_dirs() {
        let tmp = TempDir::new().unwrap();
        make_valid_project(tmp.path());
        NexaProject::load(tmp.path()).unwrap();
        assert!(tmp
            .path()
            .join("modules")
            .join("core")
            .join("src")
            .join("test")
            .is_dir());
    }

    #[test]
    fn load_missing_project_json() {
        let tmp = TempDir::new().unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingProjectJson(_)));
    }

    #[test]
    fn load_no_modules_returns_error() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","modules":[]}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"\n").unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::NoModules));
    }

    #[test]
    fn load_unknown_main_module_returns_error() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("nexa-compiler.yaml"),
            "version: \"0.1\"\nmain_module: \"nonexistent\"\n",
        )
        .unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::UnknownMainModule(_)));
    }

    #[test]
    fn load_missing_module_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"\n").unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingModuleJson(..)));
    }

    #[test]
    fn load_missing_entry_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","modules":["core"]}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"\n").unwrap();
        let src_main = tmp
            .path()
            .join("modules")
            .join("core")
            .join("src")
            .join("main");
        fs::create_dir_all(&src_main).unwrap();
        fs::write(
            tmp.path().join("modules").join("core").join("module.json"),
            r#"{"name":"core","main":"absent.nx"}"#,
        )
        .unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingModuleEntryFile(..)));
    }
}
