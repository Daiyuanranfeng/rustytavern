//! The module keeps the long-term memory of a chat: a rolling summary and a list of facts.
//! The summary belongs to one chat branch. The facts belong to one character and one persona.
//! The model updates both after each `memory_interval` new messages. The user can edit both.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::session::{
    Message, Role, Session,
    prompt::{Contribution, Retention},
    store::StoreBackend,
};
use rustytavern_core::{Error, Result};

/// The store namespace of the memory records.
pub const NAMESPACE: &str = "rustytavern.memory";
/// One update reads at most this number of message bytes. A later update reads the rest.
const MAX_INPUT_BYTES: usize = 48 * 1024;
const MAX_SUMMARY_BYTES: usize = 16 * 1024;
const MAX_FACTS: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub summary: String,
    /// The last message that the summary covers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    pub facts: Vec<String>,
}

fn keys(session: &Session) -> [String; 2] {
    [
        format!("session:{}", session.id),
        format!("facts:{}:{}", session.character_id, session.persona_id),
    ]
}

impl Memory {
    /// The function reads the memory of a session.
    pub fn load(store: &dyn StoreBackend, session: &Session) -> Result<Self> {
        let [chat, facts] = keys(session);
        let mut memory: Self = match store.state_read(NAMESPACE, &chat)? {
            Some(value) => serde_json::from_value(value)?,
            None => Self::default(),
        };
        memory.facts = match store.state_read(NAMESPACE, &facts)? {
            Some(value) => serde_json::from_value(value["facts"].clone())?,
            None => Vec::new(),
        };
        Ok(memory)
    }
    pub fn save(&self, store: &dyn StoreBackend, session: &Session) -> Result<()> {
        let [chat, facts] = keys(session);
        store.state_write(
            NAMESPACE,
            &chat,
            &json!({"summary":self.summary,"through":self.through}),
        )?;
        store.state_write(NAMESPACE, &facts, &json!({"facts":self.facts}))?;
        Ok(())
    }
    /// The method drops a summary of another branch. The summary then covers no message.
    pub fn current(mut self, history: &[Message]) -> Self {
        let known = self
            .through
            .as_ref()
            .is_some_and(|id| history.iter().any(|m| &m.id == id));
        if !known {
            self.summary.clear();
            self.through = None;
        }
        self
    }
    /// The method returns the messages after the summary.
    pub fn pending<'a>(&self, history: &'a [Message]) -> &'a [Message] {
        let start = self
            .through
            .as_ref()
            .and_then(|id| history.iter().position(|m| &m.id == id))
            .map_or(0, |index| index + 1);
        &history[start..]
    }
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.facts.is_empty()
    }
    /// The method returns the prompt sections before the history.
    pub fn contributions(&self) -> Vec<Contribution> {
        let facts = self
            .facts
            .iter()
            .map(|fact| format!("- {fact}"))
            .collect::<Vec<_>>()
            .join("\n");
        [
            (
                "summary",
                "[Summary of the earlier conversation]",
                &self.summary,
            ),
            ("facts", "[Facts to remember]", &facts),
        ]
        .into_iter()
        .filter(|(_, _, text)| !text.is_empty())
        .enumerate()
        .map(|(priority, (id, title, text))| Contribution {
            plugin_id: "memory".into(),
            section_id: id.into(),
            slot: "before_history".into(),
            priority: priority as i32,
            role: Role::System,
            text: format!("{title}\n{text}"),
            provenance: json!({"source":"memory"}),
            retention: Retention::Optional,
        })
        .collect()
    }
}

/// The function returns the messages for one update: the oldest pending messages up to the byte limit.
pub fn batch(pending: &[Message]) -> &[Message] {
    let mut bytes = 0;
    let count = pending
        .iter()
        .take_while(|m| {
            bytes += m.text.len();
            bytes <= MAX_INPUT_BYTES
        })
        .count();
    &pending[..count.max(1).min(pending.len())]
}

/// The function returns the instruction and the input of an update.
pub fn request(memory: &Memory, messages: &[Message], names: &[String; 2]) -> [String; 2] {
    let [character, user] = names;
    let instruction = format!(
        "You keep the long-term memory of a roleplay chat between {character} and {user}.
Read the current memory and the new messages. Then write the updated memory in this format:
<summary>
The story so far in at most 400 words. Keep names, places, relationships, promises, and open plot threads. Use the past tense.
</summary>
<facts>
- One lasting fact about {user}, {character}, or their relationship on each line.
</facts>
Keep each current fact that is still true. Remove each fact that the new messages make false. Write at most {MAX_FACTS} facts. Write only the memory."
    );
    let or_none = |text: String| {
        if text.is_empty() {
            "(none)".into()
        } else {
            text
        }
    };
    let facts = memory
        .facts
        .iter()
        .map(|fact| format!("- {fact}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = messages
        .iter()
        .map(|m| {
            let speaker = match m.role {
                Role::User => user,
                Role::Assistant => character,
                Role::System => "System",
            };
            format!("{speaker}: {}", m.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let input = format!(
        "Current summary:\n{}\n\nCurrent facts:\n{}\n\nNew messages:\n{lines}",
        or_none(memory.summary.clone()),
        or_none(facts)
    );
    [instruction, input]
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("<{name}>"))? + name.len() + 2;
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

/// The function reads the reply of an update. A reply without facts keeps the current facts.
pub fn parse(reply: &str, previous: &Memory, through: &str) -> Result<Memory> {
    let mut summary = tag(reply, "summary")
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            Error::new(
                "invalid_memory_reply",
                "The model reply has no <summary> block.",
            )
        })?
        .to_owned();
    if summary.len() > MAX_SUMMARY_BYTES {
        let mut end = MAX_SUMMARY_BYTES;
        while !summary.is_char_boundary(end) {
            end -= 1;
        }
        summary.truncate(end);
    }
    let facts = match tag(reply, "facts") {
        Some(text) => text
            .lines()
            .map(|line| line.trim().trim_start_matches(['-', '*']).trim())
            .filter(|line| !line.is_empty())
            .take(MAX_FACTS)
            .map(str::to_owned)
            .collect(),
        None => previous.facts.clone(),
    };
    Ok(Memory {
        summary,
        through: Some(through.to_owned()),
        facts,
    })
}

/// The function reports whether the memory of a session needs an update.
pub fn due(store: &dyn StoreBackend, session: &str) -> Result<bool> {
    let session = store.session(session)?;
    let interval = session.options.memory_interval();
    if interval == 0 {
        return Ok(false);
    }
    let history = visible(store, &session)?;
    let memory = Memory::load(store, &session)?.current(&history);
    Ok(memory.pending(&history).len() >= interval)
}

/// The function returns the history of the selected branch without the hidden messages.
pub fn visible(store: &dyn StoreBackend, session: &Session) -> Result<Vec<Message>> {
    let mut history = store.history(&session.id)?;
    history.retain(|m| !session.hidden.contains(&m.id));
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, role: Role, text: &str) -> Message {
        Message {
            id: id.into(),
            parent_id: None,
            role,
            text: text.into(),
            generation_id: None,
        }
    }

    #[test]
    fn a_summary_of_another_branch_covers_no_message() {
        let history = [
            message("a", Role::Assistant, "Hi."),
            message("b", Role::User, "Hello."),
        ];
        let memory = Memory {
            summary: "Old.".into(),
            through: Some("x".into()),
            facts: vec!["Fact.".into()],
        }
        .current(&history);
        assert_eq!(memory.summary, "");
        assert_eq!(memory.pending(&history).len(), 2);
        assert_eq!(memory.facts, ["Fact."]);
        let memory = Memory {
            through: Some("a".into()),
            ..memory
        };
        assert_eq!(memory.pending(&history)[0].id, "b");
    }

    #[test]
    fn the_reply_sets_the_summary_and_the_facts() {
        let previous = Memory {
            facts: vec!["Kept.".into()],
            ..Default::default()
        };
        let memory = parse(
            "Note.\n<summary>\nThey met.\n</summary>\n<facts>\n- Alex likes tea.\n* Lumen keeps a lighthouse.\n\n</facts>",
            &previous,
            "m9",
        )
        .unwrap();
        assert_eq!(memory.summary, "They met.");
        assert_eq!(
            memory.facts,
            ["Alex likes tea.", "Lumen keeps a lighthouse."]
        );
        assert_eq!(memory.through.as_deref(), Some("m9"));
        let kept = parse("<summary>S</summary>", &previous, "m9").unwrap();
        assert_eq!(kept.facts, ["Kept."]);
        assert_eq!(
            parse("No memory.", &previous, "m9").unwrap_err().code,
            "invalid_memory_reply"
        );
    }

    #[test]
    fn one_update_reads_the_oldest_messages_up_to_the_limit() {
        let long = "x".repeat(MAX_INPUT_BYTES / 2 + 1);
        let pending = [
            message("a", Role::User, &long),
            message("b", Role::Assistant, &long),
        ];
        assert_eq!(batch(&pending).len(), 1);
        assert_eq!(batch(&pending[..0]).len(), 0);
    }
}
