use super::{PromptPlan, Section};
use rustytavern_core::{Error, Result, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    #[default]
    Required,
    Optional,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub plugin_id: String,
    pub section_id: String,
    pub slot: String,
    pub priority: i32,
    pub role: Role,
    pub text: String,
    #[serde(default)]
    pub provenance: Value,
    #[serde(default)]
    pub retention: Retention,
}

pub fn apply_contributions(plan: &mut PromptPlan, contributions: &[Contribution]) -> Result<()> {
    if contributions.is_empty() {
        return Ok(());
    }
    let mut additions = contributions.to_vec();
    additions.sort_by(|a, b| {
        (&a.slot, a.priority, &a.plugin_id, &a.section_id).cmp(&(
            &b.slot,
            b.priority,
            &b.plugin_id,
            &b.section_id,
        ))
    });
    for slot in [
        "before_character",
        "after_character",
        "before_history",
        "after_history",
    ] {
        let mut sections = Vec::new();
        for contribution in additions.iter().filter(|c| c.slot == slot) {
            if contribution.role != Role::System {
                return Err(Error::new(
                    "invalid_prompt_role",
                    "The core-v1 contribution slots accept system text only.",
                ));
            }
            let id = format!(
                "plugin:{}:{}",
                contribution.plugin_id, contribution.section_id
            );
            if plan.sections.iter().any(|s| s.id == id) {
                return Err(Error::new(
                    "invalid_section_id",
                    "The prompt contains a duplicate contribution ID.",
                ));
            }
            sections.push(Section {
                id,
                role: contribution.role.clone(),
                text: contribution.text.clone(),
                source: contribution.plugin_id.clone(),
                mandatory: contribution.retention == Retention::Required,
                units: contribution.text.len().saturating_add(4),
            });
        }
        let index = match slot {
            "before_character" => plan
                .sections
                .iter()
                .position(|s| s.source == "character" || s.id == "persona"),
            "after_character" => plan
                .sections
                .iter()
                .position(|s| s.id == "persona")
                .map(|i| i + 1),
            "before_history" => plan
                .sections
                .iter()
                .position(|s| s.source == "history" || s.id == "current_user"),
            _ => plan.sections.iter().position(|s| s.id == "post_history"),
        }
        .unwrap_or(plan.sections.len());
        plan.sections.splice(index..index, sections);
    }
    plan.refresh()
}
