mod contribution;
pub mod phases;
pub use contribution::{Contribution, Retention, apply_contributions};

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustytavern_card::{Card, WorldBook};
use rustytavern_core::{Error, Message, Persona, Result, Role, digest};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptOptions {
    pub context_limit: usize,
    pub output_limit: usize,
    pub protocol_margin: usize,
    pub lore_budget: usize,
    pub example_budget: usize,
    pub main_instruction: String,
    pub post_history_instruction: String,
    pub note: String,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self {
            context_limit: 16384,
            output_limit: 1024,
            protocol_margin: 128,
            lore_budget: 2048,
            example_budget: 1024,
            main_instruction: "You act as {{char}}. You keep the character's identity and voice."
                .into(),
            post_history_instruction: String::new(),
            note: String::new(),
        }
    }
}

pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
    fn mode(&self) -> &str;
    fn message_overhead(&self) -> usize {
        4
    }
}

#[derive(Debug, Default)]
pub struct ConservativeCounter;
impl TokenCounter for ConservativeCounter {
    fn count(&self, text: &str) -> usize {
        text.len()
    }
    fn mode(&self) -> &str {
        "estimated_utf8_bytes"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub role: Role,
    pub text: String,
    pub source: String,
    pub mandatory: bool,
    pub units: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Omission {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptPlan {
    pub profile: String,
    pub sections: Vec<Section>,
    pub lore_ids: Vec<String>,
    pub history_ids: Vec<String>,
    pub omissions: Vec<Omission>,
    pub warnings: Vec<String>,
    pub count_mode: String,
    pub input_units: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialized_input_units: Option<usize>,
    pub output_reserve: usize,
    pub protocol_margin: usize,
    pub hash: String,
}

impl PromptPlan {
    fn groups(&self, source: &str) -> Vec<Vec<usize>> {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, section) in self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.source == source)
        {
            let new = groups.last().and_then(|g| g.last()).is_none_or(|previous| {
                let previous = &self.sections[*previous];
                match source {
                    "history" => section.role == Role::User,
                    "example" => {
                        section.id.rsplit_once(':').map(|v| v.0)
                            != previous.id.rsplit_once(':').map(|v| v.0)
                    }
                    _ => true,
                }
            });
            if new {
                groups.push(Vec::new());
            }
            groups.last_mut().expect("The group exists.").push(i);
        }
        groups
    }
    pub fn refresh(&mut self) -> Result<()> {
        let mut ids = BTreeSet::new();
        if self
            .sections
            .iter()
            .any(|s| s.id.is_empty() || !ids.insert(s.id.as_str()))
        {
            return Err(Error::new(
                "invalid_section_id",
                "The prompt section IDs are empty or duplicated.",
            ));
        }
        self.history_ids.retain(|id| {
            self.sections
                .iter()
                .any(|s| s.id == format!("history:{id}"))
        });
        self.lore_ids
            .retain(|id| self.sections.iter().any(|s| s.id == format!("lore:{id}")));
        self.input_units = self.sections.iter().map(|s| s.units).sum();
        self.serialized_input_units = None;
        self.hash = digest(&serde_json::to_vec(&self.sections)?);
        Ok(())
    }
    pub fn select_context(&mut self, options: &PromptOptions) -> Result<()> {
        self.refresh()?;
        if options.context_limit == 0 || options.output_limit == 0 {
            return Err(Error::new(
                "invalid_context_limit",
                "Context and output limits must be positive.",
            ));
        }
        let mut selected = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.mandatory)
            .map(|(i, _)| i)
            .collect::<BTreeSet<_>>();
        for source in ["history", "example"] {
            for group in self.groups(source) {
                if group.iter().any(|i| selected.contains(i)) {
                    selected.extend(group);
                }
            }
        }
        let required = selected
            .iter()
            .map(|i| self.sections[*i].units)
            .sum::<usize>();
        let mut remaining = options
            .context_limit
            .checked_sub(options.output_limit)
            .and_then(|n| n.checked_sub(options.protocol_margin))
            .and_then(|n| n.checked_sub(required))
            .ok_or_else(|| {
                Error::new(
                    "mandatory_context_overflow",
                    "The required prompt exceeds the input budget.",
                )
            })?;
        for (i, section) in self.sections.iter().enumerate().filter(|(_, s)| {
            !s.mandatory && !matches!(s.source.as_str(), "lore" | "example" | "history")
        }) {
            if section.units <= remaining {
                selected.insert(i);
                remaining -= section.units;
            } else {
                self.omissions.push(Omission {
                    id: section.id.clone(),
                    reason: "contribution_budget".into(),
                });
            }
        }
        for (source, budget) in [
            ("lore", options.lore_budget),
            ("example", options.example_budget),
            ("history", usize::MAX),
        ] {
            let mut groups = self.groups(source);
            if source == "lore" {
                groups.sort_by_key(|g| {
                    self.lore_ids
                        .iter()
                        .position(|id| self.sections[g[0]].id == format!("lore:{id}"))
                        .unwrap_or(usize::MAX)
                });
            }
            if source == "history" {
                groups.reverse();
            }
            let mut used = 0usize;
            let mut full = false;
            for group in groups {
                if group.iter().all(|i| selected.contains(i)) {
                    continue;
                }
                let units = group.iter().map(|i| self.sections[*i].units).sum::<usize>();
                if !full && units <= remaining && units <= budget.saturating_sub(used) {
                    remaining -= units;
                    used += units;
                    selected.extend(group);
                } else {
                    full |= source == "history";
                    let first = &self.sections[group[0]];
                    let ids = if source == "example" {
                        vec![
                            first
                                .id
                                .rsplit_once(':')
                                .map_or(first.id.clone(), |(id, _)| id.into()),
                        ]
                    } else {
                        group
                            .iter()
                            .map(|i| {
                                self.sections[*i]
                                    .id
                                    .strip_prefix(&format!("{source}:"))
                                    .unwrap_or(&self.sections[*i].id)
                                    .to_owned()
                            })
                            .collect()
                    };
                    self.omissions.extend(ids.into_iter().map(|id| Omission {
                        id,
                        reason: format!("{source}_budget"),
                    }));
                }
            }
        }
        let mut index = 0;
        self.sections.retain(|_| {
            let keep = selected.contains(&index);
            index += 1;
            keep
        });
        self.refresh()
    }
    pub fn trim_optional(&mut self, reason: &str) -> Result<()> {
        let removable = |group: &Vec<usize>| group.iter().all(|i| !self.sections[*i].mandatory);
        let group = self
            .groups("history")
            .into_iter()
            .find(removable)
            .or_else(|| self.groups("example").into_iter().rev().find(removable))
            .or_else(|| self.groups("lore").into_iter().rev().find(removable))
            .or_else(|| {
                self.sections
                    .iter()
                    .rposition(|s| {
                        !s.mandatory && !matches!(s.source.as_str(), "history" | "example" | "lore")
                    })
                    .map(|i| vec![i])
            })
            .ok_or_else(|| {
                Error::new(
                    "mandatory_context_overflow",
                    "The required prompt exceeds the input budget.",
                )
            })?;
        self.omissions.extend(group.iter().map(|i| Omission {
            id: self.sections[*i].id.clone(),
            reason: reason.into(),
        }));
        for i in group.into_iter().rev() {
            self.sections.remove(i);
        }
        self.refresh()
    }
}

/// A profile receives immutable data and returns an inspectable plan.
pub trait PromptProfile: Send + Sync {
    fn id(&self) -> &str;
    fn draft(&self, input: &PromptInput<'_>) -> Result<PromptPlan>;
    fn draft_with_worldbooks(
        &self,
        input: &PromptInput<'_>,
        books: &[WorldBook],
    ) -> Result<PromptPlan> {
        if !books.is_empty() {
            return Err(Error::new(
                "unsupported_worldbook_profile",
                "The prompt profile cannot use standalone worldbooks.",
            ));
        }
        self.draft(input)
    }
    fn select_context(&self, plan: &mut PromptPlan, options: &PromptOptions) -> Result<()> {
        plan.select_context(options)
    }
    fn compile(&self, input: &PromptInput<'_>) -> Result<PromptPlan> {
        let mut plan = self.draft(input)?;
        self.select_context(&mut plan, input.options)?;
        Ok(plan)
    }
}

pub struct PromptInput<'a> {
    pub card: &'a Card,
    pub persona: &'a Persona,
    pub history: &'a [Message],
    pub message: &'a str,
    pub options: &'a PromptOptions,
    pub accept_reduced: bool,
    pub counter: &'a dyn TokenCounter,
}

pub struct CoreProfile;
impl PromptProfile for CoreProfile {
    fn id(&self) -> &str {
        "core-v1"
    }
    fn draft(&self, input: &PromptInput<'_>) -> Result<PromptPlan> {
        build_core(input, &[])
    }
    fn draft_with_worldbooks(
        &self,
        input: &PromptInput<'_>,
        books: &[WorldBook],
    ) -> Result<PromptPlan> {
        build_core(input, books)
    }
}

pub fn expand(template: &str, character: &str, user: &str, original: Option<&str>) -> String {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        if start > 0 && rest.as_bytes()[start - 1] == b'\\' {
            output.push_str(&rest[..start - 1]);
            if let Some(end) = rest[start..].find("}}") {
                output.push_str(&rest[start..start + end + 2]);
                rest = &rest[start + end + 2..];
            } else {
                output.push_str(&rest[start..]);
                return output;
            }
            continue;
        }
        output.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let Some(end) = tail.find("}}") else {
            output.push_str(&rest[start..]);
            return output;
        };
        match &tail[..end] {
            "char" => output.push_str(character),
            "user" => output.push_str(user),
            "original" if original.is_some() => output.push_str(original.unwrap_or_default()),
            _ => output.push_str(&rest[start..start + end + 4]),
        }
        rest = &tail[end + 2..];
    }
    output.push_str(rest);
    output
}

pub fn instruction(default: &str, override_text: &str, character: &str, user: &str) -> String {
    let base = expand(default, character, user, None);
    if override_text.is_empty() {
        base
    } else {
        expand(override_text, character, user, Some(&base))
    }
}

#[derive(Debug, Clone)]
pub struct LoreEntry {
    pub id: String,
    pub content: String,
    pub position: String,
    pub order: i64,
    pub source_index: usize,
}

pub fn matching_lore(
    book: &Value,
    messages: &[String],
    character: &str,
    user: &str,
) -> Vec<LoreEntry> {
    let depth = book["scan_depth"].as_u64().unwrap_or(10) as usize;
    let scan = messages[messages.len().saturating_sub(depth)..].join("\n");
    let mut found = Vec::new();
    if let Some(entries) = book["entries"].as_array() {
        for (index, entry) in entries.iter().enumerate() {
            if entry["enabled"].as_bool() == Some(false) {
                continue;
            }
            let constant = entry["constant"].as_bool().unwrap_or(false);
            let case_sensitive = entry["case_sensitive"].as_bool().unwrap_or(false);
            let matches = |key: &Value| {
                let Some(key) = key.as_str().filter(|k| !k.is_empty()) else {
                    return false;
                };
                if case_sensitive {
                    scan.contains(key)
                } else {
                    scan.to_lowercase().contains(&key.to_lowercase())
                }
            };
            let primary = depth > 0
                && entry["keys"]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(matches));
            let secondary = !entry["selective"].as_bool().unwrap_or(false)
                || entry["secondary_keys"]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(matches));
            if !(constant || primary && secondary) {
                continue;
            }
            let id = match &entry["id"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => index.to_string(),
            };
            found.push(LoreEntry {
                id,
                content: expand(
                    entry["content"].as_str().unwrap_or_default(),
                    character,
                    user,
                    None,
                ),
                position: entry["position"]
                    .as_str()
                    .unwrap_or("before_char")
                    .to_owned(),
                order: entry["insertion_order"].as_i64().unwrap_or(index as i64),
                source_index: index,
            });
        }
    }
    found.sort_by_key(|entry| (entry.order, entry.source_index));
    found
}

pub fn example_blocks(text: &str, character: &str, user: &str) -> Result<Vec<Vec<(Role, String)>>> {
    let mut blocks = Vec::new();
    let mut messages: Vec<(Role, String)> = Vec::new();
    for line in text.lines() {
        if line.trim() == "<START>" {
            if !messages.is_empty() {
                blocks.push(std::mem::take(&mut messages));
            }
            continue;
        }
        let role_line = [
            ("{{char}}:", Role::Assistant),
            ("{{user}}:", Role::User),
            (&format!("{character}:"), Role::Assistant),
            (&format!("{user}:"), Role::User),
        ]
        .into_iter()
        .find_map(|(prefix, role)| {
            line.strip_prefix(prefix)
                .map(|text| (role, text.trim_start().to_owned()))
        });
        if let Some((role, text)) = role_line {
            messages.push((role, expand(&text, character, user, None)));
        } else if let Some((_, last)) = messages.last_mut() {
            last.push('\n');
            last.push_str(&expand(line, character, user, None));
        } else if !line.trim().is_empty() {
            return Err(Error::new(
                "invalid_examples",
                "Each example requires a character or user speaker prefix.",
            ));
        }
    }
    if !messages.is_empty() {
        blocks.push(messages);
    }
    Ok(blocks)
}

pub fn compile(input: &PromptInput<'_>) -> Result<PromptPlan> {
    CoreProfile.compile(input)
}

fn build_core(input: &PromptInput<'_>, books: &[WorldBook]) -> Result<PromptPlan> {
    let PromptInput {
        card,
        persona,
        history,
        message,
        options,
        accept_reduced,
        counter,
    } = input;
    card.ensure_compatible(*accept_reduced)?;
    let name = card.name();
    let user = &persona.name;
    let make = |id: &str, role: Role, text: String, source: &str, mandatory: bool| {
        let units = counter
            .count(&text)
            .saturating_add(counter.message_overhead());
        Section {
            id: id.to_owned(),
            role,
            text,
            source: source.to_owned(),
            mandatory,
            units,
        }
    };
    let mut sections = Vec::new();
    let main = instruction(
        &options.main_instruction,
        card.text("system_prompt"),
        name,
        user,
    );
    if !main.is_empty() {
        sections.push(make("main", Role::System, main, "instruction", true));
    }
    for field in ["description", "personality", "scenario"] {
        let text = expand(card.text(field), name, user, None);
        if !text.is_empty() {
            sections.push(make(field, Role::System, text, "character", true));
        }
    }
    sections.push(make(
        "persona",
        Role::System,
        format!("{}\n{}", persona.name, persona.description),
        "persona",
        true,
    ));
    if !options.note.is_empty() {
        sections.push(make(
            "note",
            Role::System,
            options.note.clone(),
            "manual_note",
            true,
        ));
    }
    let post = instruction(
        &options.post_history_instruction,
        card.text("post_history_instructions"),
        name,
        user,
    );
    if !post.is_empty() {
        sections.push(make(
            "post_history",
            Role::System,
            post,
            "instruction",
            true,
        ));
    }
    let current = make(
        "current_user",
        Role::User,
        (*message).to_owned(),
        "current_user",
        true,
    );
    let mut scan_messages = history.iter().map(|m| m.text.clone()).collect::<Vec<_>>();
    scan_messages.push((*message).to_owned());
    let mut lore = Vec::new();
    let mut entries = matching_lore(&card.data()["character_book"], &scan_messages, name, user);
    let mut warnings: Vec<String> = card
        .diagnostics
        .iter()
        .map(|d| format!("{}:{}", d.code, d.field))
        .collect();
    for book in books {
        book.validate()?;
        let document = book.document();
        let data = if document.get("spec").is_some() {
            &document["data"]
        } else {
            document
        };
        if !data["entries"].is_array() {
            return Err(Error::new(
                "unsupported_worldbook_profile",
                "The core-v1 profile cannot execute native World Info rules.",
            ));
        }
        let mut diagnostics = rustytavern_card::compatibility(
            &serde_json::json!({"data":{"character_book":data,"extensions":data["extensions"]}}),
        );
        if !data["token_budget"].is_null() {
            diagnostics.push(rustytavern_card::Diagnostic {
                code: "unsupported_lore_rule".into(),
                field: "data.character_book.token_budget".into(),
                severity: rustytavern_card::Severity::Unsupported,
            });
        }
        if !*accept_reduced
            && diagnostics
                .iter()
                .any(|d| d.severity == rustytavern_card::Severity::Unsupported)
        {
            return Err(Error::new(
                "unsupported_behavior",
                "The worldbook requires unsupported behavior. The session requires explicit acceptance of the reduced profile.",
            ));
        }
        warnings.extend(
            diagnostics
                .iter()
                .map(|d| format!("{}:{}:{}", book.id(), d.code, d.field)),
        );
        entries.extend(
            matching_lore(data, &scan_messages, name, user)
                .into_iter()
                .map(|mut entry| {
                    entry.id = format!("{}:{}", book.id(), entry.id);
                    entry
                }),
        );
    }
    for entry in entries {
        let section = make(
            &format!("lore:{}", entry.id),
            Role::System,
            entry.content.clone(),
            "lore",
            false,
        );
        lore.push((entry, section));
    }
    let mut examples = Vec::new();
    for (i, block) in example_blocks(card.text("mes_example"), name, user)?
        .into_iter()
        .enumerate()
    {
        let block_sections = block
            .into_iter()
            .enumerate()
            .map(|(j, (role, text))| {
                make(&format!("example:{i}:{j}"), role, text, "example", false)
            })
            .collect::<Vec<_>>();
        examples.extend(block_sections);
    }
    let lore_ids = lore.iter().map(|(entry, _)| entry.id.clone()).collect();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (entry, section) in lore {
        if entry.position == "after_char" {
            after.push(section);
        } else {
            before.push(section);
        }
    }
    let main_index = usize::from(sections.first().is_some_and(|s| s.id == "main"));
    sections.splice(main_index..main_index, before);
    let persona_index = sections
        .iter()
        .position(|s| s.id == "persona")
        .unwrap_or(sections.len());
    sections.splice(persona_index + 1..persona_index + 1, after);
    let post = sections
        .iter()
        .position(|s| s.id == "post_history")
        .map(|i| sections.remove(i));
    sections.extend(examples);
    let mut history_ids = Vec::new();
    for node in *history {
        history_ids.push(node.id.clone());
        sections.push(make(
            &format!("history:{}", node.id),
            node.role.clone(),
            node.text.clone(),
            "history",
            false,
        ));
    }
    sections.push(current);
    if let Some(post) = post {
        sections.push(post);
    }
    let input_units = sections.iter().map(|s| s.units).sum();
    let hash = digest(&serde_json::to_vec(&sections)?);
    Ok(PromptPlan {
        profile: "core-v1".into(),
        sections,
        lore_ids,
        history_ids,
        omissions: Vec::new(),
        warnings,
        count_mode: counter.mode().to_owned(),
        input_units,
        serialized_input_units: None,
        output_reserve: options.output_limit,
        protocol_margin: options.protocol_margin,
        hash,
    })
}
