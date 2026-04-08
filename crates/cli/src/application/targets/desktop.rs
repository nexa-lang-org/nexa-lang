use crate::application::project::{ModuleConfig, NexaProject, Platform};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DesktopBuildError {
    #[error("desktop target not yet implemented for platform '{0}'")]
    NotImplemented(String),
}

/// Stub desktop build — not yet implemented.
///
/// Returns [`DesktopBuildError::NotImplemented`] for every platform until a
/// native GUI backend (e.g. Tauri, egui) is wired in.
pub fn build(
    _project: &NexaProject,
    _module: &ModuleConfig,
    _entry: &Path,
    platform: &Platform,
) -> Result<(), DesktopBuildError> {
    Err(DesktopBuildError::NotImplemented(
        platform.as_str().to_string(),
    ))
}
