//! CLI command handlers — each sub-module owns one responsibility area.

mod build;
mod config;
mod doctor;
mod init;
mod module;
mod registry;
mod test;
mod token;

pub use build::{build, package, run};
pub use config::{config_get, config_list, config_set, theme_add, theme_list, theme_remove};
pub use doctor::{doctor, update};
pub use init::init;
pub use module::module_add;
pub use registry::{info, install, login, publish, register, search};
pub use test::test;
pub use token::{token_create, token_list, token_revoke};

use crate::application::project::NexaProject;
use crate::infrastructure::ui;
use std::path::PathBuf;

/// Default public registry URL.
pub(crate) const DEFAULT_REGISTRY: &str = "https://registry.nexa-lang.org";

/// Load a `NexaProject` from `dir` (defaults to `.`), exiting on error.
pub fn load_project(dir: Option<PathBuf>) -> NexaProject {
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    NexaProject::load(&dir).unwrap_or_else(|e| ui::die(e.to_string()))
}
