use super::{BlockPosition, PromptBlock, PromptOptions};
use rustytavern_core::{Error, Result, Role};
use serde_json::Value;

/// The function reads a SillyTavern chat completion preset into the prompt options.
/// Tavo and RisuAI read the same format.
/// The preset replaces the main instruction, the post-history instruction, the prefill, and the blocks.
/// The `main` prompt sets the main instruction. The `jailbreak` prompt sets the post-history instruction.
/// The other prompts with text become blocks. The markers between them set the block positions.
pub fn import_sillytavern(preset: &Value, mut options: PromptOptions) -> Result<PromptOptions> {
    let prompts = preset["prompts"].as_array().ok_or_else(|| {
        Error::new(
            "invalid_preset",
            "The file is not a SillyTavern chat completion preset.",
        )
    })?;
    let orders = preset["prompt_order"].as_array().into_iter().flatten();
    let order = ["100001", "100000"]
        .iter()
        .find_map(|id| {
            orders
                .clone()
                .find(|o| o["character_id"].to_string().trim_matches('"') == *id)
        })
        .or_else(|| orders.clone().next())
        .and_then(|o| o["order"].as_array())
        .map(|order| {
            order
                .iter()
                .map(|e| {
                    (
                        e["identifier"].clone(),
                        e["enabled"].as_bool() != Some(false),
                    )
                })
                .collect()
        })
        .unwrap_or_else(|| {
            prompts
                .iter()
                .map(|p| {
                    (
                        p["identifier"].clone(),
                        p["enabled"].as_bool() != Some(false),
                    )
                })
                .collect::<Vec<_>>()
        });
    options.main_instruction.clear();
    options.post_history_instruction.clear();
    options.post_history_role = Role::System;
    options.prefill = preset["assistant_prefill"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    options.blocks.clear();
    let mut position = BlockPosition::BeforeCharacter;
    for (identifier, enabled) in order {
        let id = identifier.as_str().unwrap_or_default();
        match id {
            "worldInfoBefore" | "personaDescription" | "charDescription" | "charPersonality"
            | "scenario" | "worldInfoAfter" => position = BlockPosition::AfterCharacter,
            "dialogueExamples" => position = BlockPosition::BeforeHistory,
            "chatHistory" => position = BlockPosition::AfterHistory,
            _ => {}
        }
        let Some(prompt) = prompts.iter().find(|p| p["identifier"] == identifier) else {
            continue;
        };
        let text = prompt["content"].as_str().unwrap_or_default();
        if prompt["marker"].as_bool() == Some(true) || text.is_empty() {
            continue;
        }
        let role = match prompt["role"].as_str() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => Role::System,
        };
        match id {
            "main" if enabled => options.main_instruction = text.to_owned(),
            "jailbreak" if enabled => {
                options.post_history_instruction = text.to_owned();
                options.post_history_role = role;
            }
            "main" | "jailbreak" => {}
            _ => {
                let depth = prompt["injection_position"].as_u64() == Some(1);
                options.blocks.push(PromptBlock {
                    name: prompt["name"].as_str().unwrap_or(id).to_owned(),
                    text: text.to_owned(),
                    role,
                    position: if depth {
                        BlockPosition::Depth
                    } else {
                        position
                    },
                    depth: if depth {
                        prompt["injection_depth"].as_u64().unwrap_or(4) as usize
                    } else {
                        0
                    },
                    enabled,
                });
            }
        }
    }
    Ok(options)
}
