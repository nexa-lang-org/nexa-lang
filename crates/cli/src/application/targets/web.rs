use crate::application::project::{ModuleConfig, NexaProject, Platform};
use nexa_compiler::compile_project_file;
use std::{fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WebBuildError {
    #[error("compilation failed: {0}")]
    Compile(String),
    #[error("IO error writing dist: {0}")]
    Io(#[from] std::io::Error),
}

/// Compile a web/package module to HTML + JS and write output to
/// `<root>/dist/<module>/browser/`.
///
/// # Example
///
/// ```no_run
/// use nexa::application::targets::web;
/// ```
pub fn build(
    project: &NexaProject,
    module: &ModuleConfig,
    entry: &Path,
) -> Result<(), WebBuildError> {
    let src_root = entry.parent().unwrap_or(entry);

    let result = compile_project_file(entry, src_root, project.root(), &module.name)
        .map_err(|e| WebBuildError::Compile(e.to_string()))?;

    let out_dir = project.dist_platform_dir(&module.name, &Platform::Browser);
    fs::create_dir_all(&out_dir)?;
    fs::write(out_dir.join("index.html"), &result.html)?;
    fs::write(out_dir.join("app.js"), &result.js)?;

    Ok(())
}
