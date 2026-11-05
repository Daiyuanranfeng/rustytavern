//! The `tools.skills` plugin reads Agent Skills and adds the skill catalog to the prompt.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::definition;
use crate::{
    Error, Result,
    agent::{AgentTool, ToolSet},
    plugin::PluginContext,
    prompt::{Contribution, Retention},
    provider::ToolDefinition,
};
use rustytavern_core::{Role, bounded_read, checked_file, valid_id};

const MAX_SKILLS: usize = 128;
const MAX_SKILL_BYTES: usize = 256 * 1024;

/// The `tools.skills` plugin registers `skill` and adds the skill catalog to the prompt.
pub(crate) fn activate(context: &mut PluginContext, data_dir: Option<&Path>) -> Result<()> {
    context.authorize("skills.read")?;
    let skills = match data_dir {
        Some(data_dir) => Skills::load(&data_dir.join("skills"))?,
        None => Skills(BTreeMap::new()),
    };
    let mut tools: ToolSet = Vec::new();
    if !skills.0.is_empty() {
        context.contribute(Contribution {
            plugin_id: String::new(),
            section_id: "skills".into(),
            slot: "before_history".into(),
            priority: 0,
            role: Role::System,
            text: skills.catalog(),
            provenance: Value::Null,
            retention: Retention::Optional,
        })?;
        tools.push(Arc::new(skills));
    }
    context.register("tools.skills/1", Arc::new(tools))
}

/// Each skill uses the Agent Skills layout: `skills/<name>/SKILL.md`.
struct Skills(BTreeMap<String, (String, std::path::PathBuf)>);
impl Skills {
    fn load(root: &Path) -> Result<Self> {
        let mut skills = BTreeMap::new();
        if !root.is_dir() {
            return Ok(Self(skills));
        }
        for entry in std::fs::read_dir(root)? {
            let directory = entry?.path();
            let Some(name) = directory.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !valid_id(name) || !directory.join("SKILL.md").is_file() {
                continue;
            }
            let text = String::from_utf8(bounded_read(
                &checked_file(&directory, "SKILL.md")?,
                MAX_SKILL_BYTES,
            )?)
            .map_err(Error::context(
                "invalid_skill",
                "The skill is not UTF-8 text.",
            ))?;
            let field = |key: &str| {
                let header = text.strip_prefix("---")?.split("\n---").next()?;
                header.lines().find_map(|line| {
                    let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
                    Some(value.trim_matches(['"', '\'']).to_owned())
                })
            };
            let description = field("description").unwrap_or_default();
            let name = field("name").unwrap_or_else(|| name.to_owned());
            if valid_id(&name) && skills.len() < MAX_SKILLS {
                skills.insert(name, (description, directory));
            }
        }
        Ok(Self(skills))
    }
}
impl Skills {
    fn catalog(&self) -> String {
        let list = self
            .0
            .iter()
            .map(|(name, (description, _))| format!("- {name}: {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Skill catalog. A skill is a reusable procedure. Call the skill tool to read a skill before you do a task that it covers.\n{list}"
        )
    }
}
#[async_trait]
impl AgentTool for Skills {
    fn definition(&self) -> ToolDefinition {
        definition(
            "skill",
            "Read one skill from the skill catalog.",
            json!({"type":"object","properties":{"name":{"type":"string","enum":self.0.keys().collect::<Vec<_>>()},"path":{"type":"string","description":"A file inside the skill directory. Omit it to read SKILL.md."}},"required":["name"],"additionalProperties":false}),
        )
    }
    async fn call(&self, arguments: Value, _: CancellationToken) -> Result<String> {
        let (_, directory) = self
            .0
            .get(arguments["name"].as_str().unwrap_or_default())
            .ok_or_else(|| Error::new("not_found", "The skill does not exist."))?;
        let path = arguments["path"].as_str().unwrap_or("SKILL.md");
        let bytes = bounded_read(&checked_file(directory, path)?, MAX_SKILL_BYTES)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
