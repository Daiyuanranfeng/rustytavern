use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustytavern::{
    config::Application,
    session::{Generation, GenerationStatus},
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tui_textarea::TextArea;

pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("/setup", "Configure a model provider"),
    ("/model", "Select or edit a provider"),
    ("/models", "Discover advertised models"),
    ("/character", "Select or import a character"),
    ("/worldbook", "Import or bind worldbooks"),
    ("/persona", "Select or create a persona"),
    ("/new", "Start a conversation"),
    ("/sessions", "Resume a conversation"),
    ("/config", "Inspect configuration"),
    ("/plugins", "Inspect plugins and bind services"),
    ("/extensions", "Manage product packages"),
    ("/prompt", "Inspect a complete model request"),
    ("/adopt", "Adopt current configuration"),
    ("/retry", "Retry or regenerate the turn"),
    ("/branch", "Select a conversation head"),
    ("/export", "Export the conversation"),
    ("/help", "Read commands and keys"),
    ("/quit", "Close the terminal"),
];

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Preferences {
    pub session: Option<String>,
    pub character: Option<String>,
    pub persona: Option<String>,
    pub provider: Option<String>,
}

pub(super) enum Job {
    Command(Command, After),
    Generate {
        session: String,
        message: String,
        retry: bool,
    },
}
pub(super) enum After {
    Notice,
    Card,
    Persona,
    Provider(String),
    Models(String),
    Session,
    Refresh,
    View(&'static str),
    Request(Option<PathBuf>),
}
#[derive(Clone)]
pub(super) enum FormKind {
    Models(String),
    WorldbookImport,
    WorldbookExport,
    Provider,
    Import,
    Fetch,
    Persona,
    New,
    Export,
    Prompt,
    Bind,
    Install,
    Enable(String),
    Run,
}
pub(super) enum Choice {
    Slash(&'static str),
    Select(Selection, String),
    Form(FormKind),
    Command(Command, After),
}
pub(super) enum Selection {
    DiscoveredModel(String),
    Card,
    Persona,
    Provider,
    Session,
    Branch,
    Enable,
    Disable,
    Uninstall,
}
pub(super) struct Field {
    pub name: &'static str,
    pub editor: TextArea<'static>,
}
pub(super) enum Overlay {
    Menu {
        title: String,
        items: Vec<(String, Choice)>,
        selected: usize,
    },
    Form {
        title: String,
        kind: FormKind,
        fields: Vec<Field>,
        selected: usize,
    },
    Document {
        title: String,
        text: String,
        scroll: u16,
    },
}
pub(super) struct Ui {
    pub path: PathBuf,
    pub preferences: Preferences,
    pub composer: TextArea<'static>,
    pub overlay: Option<Overlay>,
    pub transcript: String,
    pub reply: String,
    pub title: String,
    pub notice: String,
    pub busy: bool,
    pub exit: bool,
    pub scroll: u16,
    pub tick: usize,
    pub pending: Option<Job>,
    pub new_requested: bool,
}

pub(super) fn text(source: &str) -> String {
    let mut characters = source
        .chars()
        .map(|ch| if ch == '\t' { ' ' } else { ch })
        .filter(|ch| !ch.is_control() || *ch == '\n');
    let mut result: String = characters.by_ref().take(128 * 1024).collect();
    if characters.next().is_some() {
        result.push_str("\nThe terminal view is limited. The export retains all data.");
    }
    result
}
fn editor(value: &str) -> TextArea<'static> {
    TextArea::from(text(value).lines().map(str::to_owned).collect::<Vec<_>>())
}
pub(super) fn field(name: &'static str, value: &str) -> Field {
    Field {
        name,
        editor: editor(value),
    }
}

impl Ui {
    pub fn open(path: PathBuf) -> Result<Self> {
        Application::open(&path)?;
        let preference_path = path.join("terminal.json");
        let (preferences, notice) = if preference_path.exists() {
            let mut bytes = Vec::new();
            std::fs::File::open(preference_path)?
                .take(65537)
                .read_to_end(&mut bytes)?;
            match serde_json::from_slice(&bytes) {
                Ok(value) if bytes.len() <= 65536 => (value, String::new()),
                _ => (
                    Preferences::default(),
                    "The terminal preferences are invalid. Use /sessions to select a conversation."
                        .into(),
                ),
            }
        } else {
            (
                Preferences::default(),
                "Use /setup, /character, /persona, then /new.".into(),
            )
        };
        let mut ui = Self {
            path,
            preferences,
            composer: editor(""),
            overlay: None,
            transcript: String::new(),
            reply: String::new(),
            title: "RustyTavern".into(),
            notice,
            busy: false,
            exit: false,
            scroll: 0,
            tick: 0,
            pending: None,
            new_requested: false,
        };
        if let Err(error) = ui.refresh() {
            ui.notice = error.to_string();
            ui.preferences.session = None;
        }
        Ok(ui)
    }
    pub fn save(&self) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.path)?;
        file.write_all(&serde_json::to_vec(&self.preferences)?)?;
        file.as_file().sync_all()?;
        file.persist(self.path.join("terminal.json")).map_err(|e| {
            Error::new("io", "The terminal preferences cannot be saved.").with_source(e.error)
        })?;
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<()> {
        let Some(id) = &self.preferences.session else {
            return Ok(());
        };
        let app = Application::open(&self.path)?;
        let session = app.store.session(id)?;
        let card = app.store.card(&session.character_id)?;
        let persona = app.store.persona(&session.persona_id)?;
        self.title = format!("{} · {} · {}", card.name(), persona.name, session.model);
        let history = app.store.history(id)?;
        let mut parts = Vec::new();
        if history.len() > 100 {
            parts.push(
                "The display shows the last 100 messages. Export retains the full history.".into(),
            );
        }
        for message in history.iter().skip(history.len().saturating_sub(100)) {
            let label = match message.role {
                rustytavern::session::Role::User => &persona.name,
                rustytavern::session::Role::Assistant => card.name(),
                _ => "System",
            };
            parts.push(format!("{label}\n{}", text(&message.text)));
        }
        let joined = parts.join("\n\n");
        let start = joined
            .char_indices()
            .rev()
            .nth(128 * 1024 - 1)
            .map_or(0, |(index, _)| index);
        self.transcript = text(&joined[start..]);
        if start > 0 {
            self.transcript.insert_str(
                0,
                "The display shows recent text. The export retains all history.\n\n",
            );
        }
        self.scroll = 0;
        Ok(())
    }
    pub fn clear_input(&mut self) {
        self.overlay = None;
        self.composer = editor("");
    }
    pub fn paste(&mut self, value: &str) {
        let target = if let Some(Overlay::Form {
            fields, selected, ..
        }) = &mut self.overlay
        {
            &mut fields[*selected].editor
        } else if self.overlay.is_none() {
            &mut self.composer
        } else {
            return;
        };
        if target.lines().iter().map(String::len).sum::<usize>() + value.len() <= 128 * 1024 {
            target.insert_str(text(value));
        } else {
            self.notice = "The terminal input exceeds 128 KiB.".into();
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            self.overlay = None;
            return Ok(());
        }
        match self.overlay.take() {
            Some(Overlay::Menu {
                title,
                mut items,
                mut selected,
            }) => {
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(items.len().saturating_sub(1)),
                    KeyCode::Enter if !items.is_empty() => {
                        let (_, choice) = items.remove(selected);
                        self.choose(choice)?;
                        return Ok(());
                    }
                    _ => {}
                }
                self.overlay = Some(Overlay::Menu {
                    title,
                    items,
                    selected,
                });
            }
            Some(Overlay::Document {
                title,
                text,
                mut scroll,
            }) => {
                match key.code {
                    KeyCode::Down | KeyCode::PageDown => scroll = scroll.saturating_add(8),
                    KeyCode::Up | KeyCode::PageUp => scroll = scroll.saturating_sub(8),
                    _ => {}
                }
                self.overlay = Some(Overlay::Document {
                    title,
                    text,
                    scroll,
                });
            }
            Some(Overlay::Form {
                title,
                kind,
                mut fields,
                mut selected,
            }) => {
                match key.code {
                    KeyCode::Tab => selected = (selected + 1) % fields.len(),
                    KeyCode::BackTab => selected = (selected + fields.len() - 1) % fields.len(),
                    KeyCode::Enter
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
                    {
                        let values = fields
                            .iter()
                            .map(|f| f.editor.lines().join("\n"))
                            .collect::<Vec<_>>();
                        match self.submit(&kind, &values) {
                            Ok(()) => return Ok(()),
                            Err(error) => self.notice = error.to_string(),
                        }
                    }
                    KeyCode::Enter => fields[selected].editor.insert_newline(),
                    _ => {
                        if fields[selected]
                            .editor
                            .lines()
                            .iter()
                            .map(String::len)
                            .sum::<usize>()
                            < 128 * 1024
                        {
                            fields[selected].editor.input(key);
                        }
                    }
                }
                self.overlay = Some(Overlay::Form {
                    title,
                    kind,
                    fields,
                    selected,
                });
            }
            None => match key.code {
                KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                KeyCode::Tab if self.composer.lines().join("\n").starts_with('/') => {
                    let prefix = self.composer.lines().join("\n");
                    self.menu(
                        "Commands",
                        COMMANDS
                            .iter()
                            .filter(|(name, _)| name.starts_with(&prefix))
                            .map(|(name, description)| {
                                (format!("{name}  {description}"), Choice::Slash(name))
                            })
                            .collect(),
                    );
                }
                KeyCode::Enter
                    if key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
                {
                    self.composer.insert_newline()
                }
                KeyCode::Enter => {
                    let value = self.composer.lines().join("\n");
                    if value.starts_with('/') {
                        self.composer = editor("");
                        if let Err(error) = self.slash(value.trim()) {
                            self.notice = error.to_string();
                        }
                    } else if !value.trim().is_empty() {
                        if let Some(session) = self.preferences.session.clone() {
                            self.pending = Some(Job::Generate {
                                session,
                                message: value,
                                retry: false,
                            });
                            self.composer = editor("");
                            self.reply.clear();
                            self.notice = "The model is replying. Ctrl+C cancels.".into();
                        } else {
                            self.notice = "Select a conversation with /new or /sessions.".into();
                        }
                    }
                }
                _ => {
                    if self.composer.lines().iter().map(String::len).sum::<usize>() < 128 * 1024 {
                        self.composer.input(key);
                    }
                }
            },
        }
        Ok(())
    }
    pub(super) fn menu(&mut self, title: &str, items: Vec<(String, Choice)>) {
        self.overlay = Some(Overlay::Menu {
            title: title.into(),
            items,
            selected: 0,
        });
    }
    pub(super) fn document(&mut self, title: &str, value: &Value) {
        self.overlay = Some(Overlay::Document {
            title: title.into(),
            text: text(&serde_json::to_string_pretty(value).unwrap_or_default()),
            scroll: 0,
        });
    }
    pub(super) fn command(&mut self, command: Command, after: After) {
        self.pending = Some(Job::Command(command, after));
        self.notice = "The application is applying the operation.".into();
    }
    pub(super) fn active(&self) -> Result<String> {
        self.preferences.session.clone().ok_or_else(|| {
            Error::new(
                "invalid_session",
                "Select a conversation with /new or /sessions.",
            )
        })
    }
    pub fn complete(&mut self, result: Result<Vec<Value>>, after: After) {
        let result = (|| -> Result<()> {
            let values = result?;
            let value = values.last().cloned().unwrap_or(Value::Null);
            match after {
                After::Models(provider) => {
                    let catalog: rustytavern::provider::catalog::ModelCatalog =
                        serde_json::from_value(value)?;
                    let items = catalog
                        .models
                        .iter()
                        .map(|model| {
                            (
                                format!(
                                    "{}  {}",
                                    text(&model.id),
                                    model
                                        .advertised
                                        .output_modalities
                                        .as_ref()
                                        .map(|values| values.join(", "))
                                        .unwrap_or_else(|| "unknown output".into())
                                ),
                                Choice::Select(
                                    Selection::DiscoveredModel(provider.clone()),
                                    model.id.clone(),
                                ),
                            )
                        })
                        .collect();
                    self.menu("Advertised models: select a provider default", items);
                    self.notice =
                        "The catalog advertises capabilities. The catalog does not verify them."
                            .into();
                    return Ok(());
                }
                After::Card => self.preferences.character = value["id"].as_str().map(str::to_owned),
                After::Persona => {
                    self.preferences.persona = value["id"].as_str().map(str::to_owned)
                }
                After::Provider(name) => self.preferences.provider = Some(name),
                After::Session => {
                    self.preferences.session = value["id"].as_str().map(str::to_owned)
                }
                After::View(title) => {
                    self.document(title, &value);
                    return Ok(());
                }
                After::Request(path) => {
                    if let Some(path) = path {
                        std::fs::write(path, serde_json::to_vec_pretty(&value)?)?;
                    }
                    self.document("Complete model request", &value);
                    return Ok(());
                }
                After::Refresh | After::Notice => {}
            }
            self.refresh()?;
            self.save()?;
            self.notice = "The operation completed.".into();
            self.advance_new()?;
            Ok(())
        })();
        if let Err(error) = result {
            self.notice = error.to_string();
        }
    }
    pub fn event(&mut self, event: RuntimeEvent) {
        if event.r#type == "text_delta" {
            if let Some(value) = event.text {
                if self.reply.len() < 128 * 1024 {
                    self.reply.push_str(&text(&value));
                }
            }
        }
    }
    pub fn finish(&mut self, result: Result<Generation>) {
        match result {
            Ok(generation) => {
                self.reply.clear();
                let _ = self.refresh();
                self.notice = if generation.status == GenerationStatus::Completed {
                    "The reply is saved.".into()
                } else {
                    self.reply = text(&generation.draft);
                    format!(
                        "{} The draft remains saved.",
                        generation
                            .error
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "The generation ended.".into())
                    )
                };
            }
            Err(error) => {
                self.notice = error.to_string();
                let _ = self.refresh();
            }
        }
    }
}

#[cfg(test)]
mod tests;
