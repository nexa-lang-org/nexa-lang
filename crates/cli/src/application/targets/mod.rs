#![allow(dead_code)]

pub mod desktop;
pub mod dispatcher;
pub mod rust;
pub mod web;

#[allow(unused_imports)]
pub use dispatcher::build_module;
