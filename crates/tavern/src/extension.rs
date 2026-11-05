//! An extension changes how an application looks, for example the message bubbles.
//! An extension cannot change agent behavior or agent context. A plugin does that.
//! Each `[extensions.NAME]` table in `config.toml` is one enabled extension.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Each color is a name, such as `magenta`, or a hex value, such as `#5f87ff`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Extension {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_bubble: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_bubble: Option<String>,
}

/// The function merges the extensions in name order. A later name overrides an earlier name.
pub fn theme(extensions: &BTreeMap<String, Extension>) -> Extension {
    let mut theme = Extension::default();
    for extension in extensions.values() {
        theme.user_bubble = extension.user_bubble.clone().or(theme.user_bubble);
        theme.assistant_bubble = extension
            .assistant_bubble
            .clone()
            .or(theme.assistant_bubble);
    }
    theme
}
