use crate::application::project::{ModuleConfig, NexaProject};
use nexa_compiler::{compile_to_ir, RustCodegen, RustCodegenError};
use std::{fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RustBuildError {
    #[error("IR compilation failed: {0}")]
    Ir(String),
    #[error("Rust codegen failed: {0}")]
    Codegen(#[from] RustCodegenError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Compile a backend/cli module to Rust source and write output to
/// `<root>/.nexa/nex_out/<module>/<platform>/`.
///
/// Outputs `main.rs` and `Cargo.toml` ready for `cargo build`.
///
/// # Example
///
/// ```no_run
/// use nexa::application::targets::rust;
/// ```
pub fn build(
    project: &NexaProject,
    module: &ModuleConfig,
    entry: &Path,
) -> Result<(), RustBuildError> {
    let src_root = entry.parent().unwrap_or(entry);

    // 1. Compile to IR
    let ir = compile_to_ir(entry, src_root, project.root(), &module.name)
        .map_err(|e| RustBuildError::Ir(e.to_string()))?;

    // 2. Generate Rust source
    let project_version = module.version.as_deref().unwrap_or("0.1.0");
    let gen = RustCodegen::new(&module.name, &project.project.name, project_version);
    let result = gen.generate(&ir)?;

    // 3. Write output to .nexa/nex_out/<module_name>/
    let out_dir = project
        .root()
        .join(".nexa")
        .join("nex_out")
        .join(&module.name);
    fs::create_dir_all(&out_dir)?;
    fs::write(out_dir.join("main.rs"), &result.main_rs)?;
    fs::write(out_dir.join("Cargo.toml"), &result.cargo_toml)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::project::NexaProject;
    use std::fs;
    use tempfile::TempDir;

    /// Set up a minimal valid Nexa project layout in `tmp` with a backend module
    /// named `my_app` whose entry file contains `source`.
    fn make_backend_project(tmp: &TempDir, source: &str) -> (NexaProject, ModuleConfig, std::path::PathBuf) {
        let root = tmp.path();

        // project.json
        fs::write(
            root.join("project.json"),
            r#"{"name":"my_app","version":"0.1.0","author":"Test","modules":["my_app"]}"#,
        )
        .unwrap();

        // nexa-compiler.yaml
        fs::write(
            root.join("nexa-compiler.yaml"),
            "version: \"0.1\"\nmain_module: \"my_app\"\n",
        )
        .unwrap();

        // module structure
        let src_main = root
            .join("modules")
            .join("my_app")
            .join("src")
            .join("main");
        fs::create_dir_all(&src_main).unwrap();

        fs::write(
            root.join("modules").join("my_app").join("module.json"),
            r#"{"name":"my_app","main":"main.nx","type":"backend"}"#,
        )
        .unwrap();

        let entry = src_main.join("main.nx");
        fs::write(&entry, source).unwrap();

        let project = NexaProject::load(root).unwrap();
        let module = project.modules.get("my_app").unwrap().clone();

        (project, module, entry)
    }

    #[test]
    fn rust_build_writes_main_rs_and_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        let source = r#"
app my_app {
    class my_app {
        main() => Void {
            Console.log("hello");
        }
    }
}
"#;
        let (project, module, entry) = make_backend_project(&tmp, source);

        let result = build(&project, &module, &entry);
        assert!(result.is_ok(), "build failed: {:?}", result);

        let out_dir = tmp
            .path()
            .join(".nexa")
            .join("nex_out")
            .join("my_app");
        assert!(out_dir.join("main.rs").exists(), "main.rs not written");
        assert!(out_dir.join("Cargo.toml").exists(), "Cargo.toml not written");

        let main_rs = fs::read_to_string(out_dir.join("main.rs")).unwrap();
        assert!(main_rs.contains("fn main()"), "expected fn main() in output");
    }

    #[test]
    fn rust_build_uses_module_version_in_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        let source = r#"
app my_app {
    class my_app {
        main() => Void {
            Console.log("versioned");
        }
    }
}
"#;
        // Override module version via direct construction after load
        let (project, mut module, entry) = make_backend_project(&tmp, source);
        module.version = Some("1.2.3".to_string());

        let result = build(&project, &module, &entry);
        assert!(result.is_ok(), "build failed: {:?}", result);

        let cargo_toml = fs::read_to_string(
            tmp.path()
                .join(".nexa")
                .join("nex_out")
                .join("my_app")
                .join("Cargo.toml"),
        )
        .unwrap();
        assert!(
            cargo_toml.contains("1.2.3"),
            "expected module version '1.2.3' in Cargo.toml, got:\n{cargo_toml}"
        );
    }
}
