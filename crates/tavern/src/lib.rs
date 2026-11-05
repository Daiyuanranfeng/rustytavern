//! RustyTavern supplies the assembled character runtime and product services.
pub mod app;
pub mod config;
pub use rustytavern_adapters::{provider, source};
pub use rustytavern_card as card;
pub use rustytavern_conversation as session;
pub use rustytavern_core::{Error, Result, digest, error};
pub use rustytavern_plugin::process as external;
pub use rustytavern_runtime::{self as runtime, driver, phases};
pub use rustytavern_store as store;
pub mod plugin {
    pub use rustytavern_plugin::process as external;
    pub use rustytavern_plugin::*;
}
pub mod prompt {
    pub use rustytavern_conversation::prompt::*;
    pub use rustytavern_runtime::phases;
}
