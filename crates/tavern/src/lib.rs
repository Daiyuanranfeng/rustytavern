//! RustyTavern assembles the product: the application, configuration, extensions, and bundled plugins.
//! The crate re-exports the lower crates under one path.
pub mod app;
pub mod config;
pub mod extension;
pub mod plugins;
pub use rustytavern_core::card;
pub use rustytavern_core::{Error, Result, digest, error};
pub use rustytavern_runtime::{
    self as runtime, agent, driver, phases, plugin, plugin::process as external, provider, source,
};
pub use rustytavern_session::{self as session, store};
pub mod prompt {
    pub use rustytavern_runtime::phases;
    pub use rustytavern_session::prompt::*;
}
