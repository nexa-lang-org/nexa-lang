use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use thiserror::Error;

// ── Erreurs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("project.json introuvable dans '{0}' — êtes-vous dans un projet Nexa ?")]
    MissingProjectJson(PathBuf),

    #[error("nexa-compiler.yaml introuvable dans '{0}'")]
    MissingCompilerYaml(PathBuf),

    #[error("répertoire src/main/ introuvable dans '{0}'")]
    MissingSrcMain(PathBuf),

    #[error("fichier d'entrée introuvable : '{0}'")]
    MissingEntryFile(PathBuf),

    #[error("lecture project.json : {0}")]
    ReadProjectJson(#[source] std::io::Error),

    #[error("parse project.json : {0}")]
    ParseProjectJson(#[source] serde_json::Error),

    #[error("lecture nexa-compiler.yaml : {0}")]
    ReadCompilerYaml(#[source] std::io::Error),

    #[error("parse nexa-compiler.yaml : {0}")]
    ParseCompilerYaml(#[source] serde_yaml::Error),
}

// ── Structs de config ─────────────────────────────────────────────────────────

/// Désérialisé depuis `project.json`
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub name: String,
    pub version: String,
    pub author: String,
    /// Nom du fichier d'entrée dans `src/main/`, ex: "app.nx"
    pub main: String,
    /// Dépendances : { "my-lib": "^1.0.0" }
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

/// Un registry privé déclaré dans `nexa-compiler.yaml`
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PrivateRegistry {
    pub url: String,
    pub key: String,
}

/// Désérialisé depuis `nexa-compiler.yaml`
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CompilerConfig {
    pub version: String,
    /// Registry public (défaut : https://registry.nexa-lang.org)
    #[serde(default)]
    pub registry: Option<String>,
    /// Registries privés avec clé d'API
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
}

// ── Projet ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NexaProject {
    root: PathBuf,
    pub project: ProjectConfig,
    pub compiler: CompilerConfig,
}

// ── Fonctions de parsing pures (testables indépendamment) ─────────────────────

pub fn parse_project_config(text: &str) -> Result<ProjectConfig, ProjectError> {
    serde_json::from_str(text).map_err(ProjectError::ParseProjectJson)
}

pub fn parse_compiler_config(text: &str) -> Result<CompilerConfig, ProjectError> {
    serde_yaml::from_str(text).map_err(ProjectError::ParseCompilerYaml)
}

// ── Implémentation ────────────────────────────────────────────────────────────

impl NexaProject {
    /// Charge et valide un projet depuis `dir`.
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

        let src_main = root.join("src").join("main");
        if !src_main.is_dir() {
            return Err(ProjectError::MissingSrcMain(root));
        }

        let entry = src_main.join(&project.main);
        if !entry.exists() {
            return Err(ProjectError::MissingEntryFile(entry));
        }

        let proj = NexaProject {
            root,
            project,
            compiler,
        };
        proj.ensure_optional_dirs();
        Ok(proj)
    }

    /// Crée silencieusement les répertoires optionnels s'ils n'existent pas encore.
    fn ensure_optional_dirs(&self) {
        for d in &[
            self.src_root().join(".nexa"),
            self.src_root().join("libs"),
            self.src_root().join("test"),
        ] {
            let _ = fs::create_dir_all(d);
        }
    }

    /// `<root>/src/`
    pub fn src_root(&self) -> PathBuf {
        self.root.join("src")
    }

    /// `<root>/src/main/<project.main>`
    pub fn entry_file(&self) -> PathBuf {
        self.src_root().join("main").join(&self.project.main)
    }

    /// `<root>/src/dist/`
    pub fn dist_dir(&self) -> PathBuf {
        self.src_root().join("dist")
    }

    /// `<root>/nexa-libs/`
    pub fn libs_dir(&self) -> PathBuf {
        self.root.join("nexa-libs")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_valid_project(dir: &Path) {
        fs::write(
            dir.join("project.json"),
            r#"{"name":"test-app","version":"0.1.0","author":"Tester","main":"app.nx"}"#,
        )
        .unwrap();
        fs::write(dir.join("nexa-compiler.yaml"), "version: \"0.1\"\n").unwrap();
        let src_main = dir.join("src").join("main");
        fs::create_dir_all(&src_main).unwrap();
        fs::write(src_main.join("app.nx"), "").unwrap();
    }

    #[test]
    fn parse_project_config_valid() {
        let json = r#"{"name":"my-app","version":"1.0.0","author":"Dev","main":"app.nx"}"#;
        let cfg = parse_project_config(json).unwrap();
        assert_eq!(cfg.name, "my-app");
        assert_eq!(cfg.main, "app.nx");
        assert!(cfg.dependencies.is_empty());
    }

    #[test]
    fn parse_project_config_dependencies_optional() {
        let json = r#"{"name":"a","version":"1","author":"b","main":"m.nx"}"#;
        let cfg = parse_project_config(json).unwrap();
        assert!(cfg.dependencies.is_empty());
    }

    #[test]
    fn parse_project_config_with_dependencies() {
        let json = r#"{"name":"a","version":"1","author":"b","main":"m.nx","dependencies":{"my-lib":"^1.0.0","other":"2.0.0"}}"#;
        let cfg = parse_project_config(json).unwrap();
        assert_eq!(
            cfg.dependencies.get("my-lib").map(String::as_str),
            Some("^1.0.0")
        );
        assert_eq!(
            cfg.dependencies.get("other").map(String::as_str),
            Some("2.0.0")
        );
    }

    #[test]
    fn parse_project_config_missing_required_field() {
        let json = r#"{"name":"a","version":"1","author":"b"}"#;
        assert!(parse_project_config(json).is_err());
    }

    #[test]
    fn parse_project_config_invalid_json() {
        assert!(parse_project_config("pas du json").is_err());
    }

    #[test]
    fn parse_compiler_config_valid() {
        let cfg = parse_compiler_config("version: \"0.1\"\n").unwrap();
        assert_eq!(cfg.version, "0.1");
        assert!(cfg.registry.is_none());
        assert!(cfg.private_registries.is_empty());
    }

    #[test]
    fn parse_compiler_config_with_registry() {
        let yaml = "version: \"0.1\"\nregistry: \"https://my.registry.com\"\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        assert_eq!(cfg.registry.as_deref(), Some("https://my.registry.com"));
    }

    #[test]
    fn parse_compiler_config_with_private_registries() {
        let yaml = "version: \"0.1\"\nprivate_registries:\n  - url: \"https://corp.reg\"\n    key: \"sk_abc\"\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        assert_eq!(cfg.private_registries.len(), 1);
        assert_eq!(cfg.private_registries[0].url, "https://corp.reg");
        assert_eq!(cfg.private_registries[0].key, "sk_abc");
    }

    #[test]
    fn compiler_config_all_registries_order() {
        let yaml = "version: \"0.1\"\nregistry: \"https://pub.reg\"\nprivate_registries:\n  - url: \"https://priv.reg\"\n    key: \"k\"\n";
        let cfg = parse_compiler_config(yaml).unwrap();
        let regs = cfg.all_registries();
        assert_eq!(regs[0].0, "https://priv.reg"); // private first
        assert_eq!(regs[1].0, "https://pub.reg"); // public second
    }

    #[test]
    fn parse_compiler_config_missing_version() {
        assert!(parse_compiler_config("autre_champ: true").is_err());
    }

    #[test]
    fn parse_compiler_config_invalid_yaml() {
        assert!(parse_compiler_config(":\n  bad:\n  yaml:").is_err());
    }

    #[test]
    fn path_helpers_are_correct() {
        let tmp = TempDir::new().unwrap();
        make_valid_project(tmp.path());
        let proj = NexaProject::load(tmp.path()).unwrap();
        assert_eq!(proj.src_root(), tmp.path().join("src"));
        assert_eq!(
            proj.entry_file(),
            tmp.path().join("src").join("main").join("app.nx")
        );
        assert_eq!(proj.dist_dir(), tmp.path().join("src").join("dist"));
        assert_eq!(proj.libs_dir(), tmp.path().join("nexa-libs"));
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
        assert!(tmp.path().join("src").join(".nexa").is_dir());
        assert!(tmp.path().join("src").join("libs").is_dir());
        assert!(tmp.path().join("src").join("test").is_dir());
    }

    #[test]
    fn load_missing_project_json() {
        let tmp = TempDir::new().unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingProjectJson(_)));
    }

    #[test]
    fn load_invalid_project_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("project.json"), "pas du json").unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"").unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::ParseProjectJson(_)));
    }

    #[test]
    fn load_missing_compiler_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","main":"m.nx"}"#,
        )
        .unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingCompilerYaml(_)));
    }

    #[test]
    fn load_missing_src_main() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","main":"m.nx"}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"").unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingSrcMain(_)));
    }

    #[test]
    fn load_missing_entry_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("project.json"),
            r#"{"name":"a","version":"1","author":"b","main":"absent.nx"}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("nexa-compiler.yaml"), "version: \"0.1\"").unwrap();
        fs::create_dir_all(tmp.path().join("src").join("main")).unwrap();
        let err = NexaProject::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingEntryFile(_)));
    }
}
