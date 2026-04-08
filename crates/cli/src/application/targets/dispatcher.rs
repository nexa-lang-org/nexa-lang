use super::{desktop, rust, web};
use crate::application::project::{AppType, ModuleConfig, NexaProject};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("web target error: {0}")]
    Web(String),
    #[error("rust target error: {0}")]
    Rust(String),
    #[error("desktop target error: {0}")]
    Desktop(String),
    #[error("unsupported target: {0}")]
    Unsupported(String),
}

/// Build all effective platforms for a single module.
///
/// Web/Rust targets are platform-agnostic; one result is returned.
/// Desktop dispatches per-platform and returns one result per platform.
pub fn build_module(
    project: &NexaProject,
    module: &ModuleConfig,
    entry: &Path,
) -> Vec<Result<(), BuildError>> {
    match module.app_type {
        AppType::Web | AppType::Package => vec![
            web::build(project, module, entry)
                .map_err(|e| BuildError::Web(e.to_string())),
        ],
        AppType::Backend | AppType::Cli => vec![
            rust::build(project, module, entry)
                .map_err(|e| BuildError::Rust(e.to_string())),
        ],
        AppType::Desktop => module
            .effective_platforms()
            .iter()
            .map(|p| {
                desktop::build(project, module, entry, p)
                    .map_err(|e| BuildError::Desktop(e.to_string()))
            })
            .collect(),
    }
}
