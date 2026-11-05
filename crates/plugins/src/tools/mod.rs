//! The tool plugins register `tools.NAME/1` services for the agent driver.
//! Each plugin requires its own grant.

#[cfg(feature = "mcp")]
pub(crate) mod mcp;
pub(crate) mod skills;
pub(crate) mod web;

use crate::provider::ToolDefinition;
use serde_json::Value;

const MAX_DESCRIPTION_BYTES: usize = 16_384;

fn definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    let mut description = description.to_owned();
    if description.len() > MAX_DESCRIPTION_BYTES {
        let mut end = MAX_DESCRIPTION_BYTES;
        while !description.is_char_boundary(end) {
            end -= 1;
        }
        description.truncate(end);
    }
    ToolDefinition {
        name: name.into(),
        description,
        parameters,
    }
}
