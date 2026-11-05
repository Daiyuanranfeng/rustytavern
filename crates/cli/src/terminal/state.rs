use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use rustytavern::{
    config::Application,
    session::{Generation, GenerationStatus},
};
use serde::{Deserialize, Serialize};
use tui_textarea::TextArea;

pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("/provider", "Connect or switch a model provider"),
    ("/model", "Choose a model from the current provider"),
    ("/character", "Select or add a character"),
    ("/persona", "Select or create your persona"),
    ("/new", "Start a chat"),
    ("/chats", "Resume a chat"),
    ("/retry", "Regenerate the last reply"),
    (
        "/settings",
        "Lorebooks, branches, export, plugins, and more",
    ),
    ("/help", "Read commands and keys"),
    ("/quit", "Close the terminal"),
];

/// The character preview has these tabs.
pub(super) const PREVIEW_TABS: &[&str] = &["Overview", "Greetings", "Lorebook", "Prompts"];

/// The namespace and key of the terminal selections in the database.
const PREFERENCES: (&str, &str) = ("rustytavern.terminal", "preferences");

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Preferences {
    pub session: Option<String>,
    pub character: Option<String>,
    pub persona: Option<String>,
    /// The `default_provider` setting in `config.toml` holds this value.
    #[serde(skip)]
    pub provider: Option<String>,
}

pub(super) enum Job {
    Command(Command, After),
    /// The job loads the model catalogs of several providers at the same time.
    Catalogs(Vec<String>),
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
    Setup(String),
    Models(Vec<String>),
    Session,
    Refresh,
    View(&'static str),
    Request(Option<PathBuf>),
    /// The download shows a preview. The address and the source fetch the card again on Enter.
    Preview(Box<[String; 2]>),
}
#[derive(Clone)]
pub(super) enum FormKind {
    WorldbookImport,
    WorldbookExport,
    Setup(String),
    Provider(String),
    Import,
    Persona,
    Export,
    Prompt,
    Bind,
    Install,
    Run,
}
pub(super) enum Choice {
    Slash(&'static str),
    Select(Selection, String),
    Form(FormKind),
    Command(Command, After),
    /// The choice lists a folder.
    Browse(PathBuf),
    /// The choice shows a character file.
    Open(PathBuf),
}
pub(super) enum Selection {
    DiscoveredModel(String),
    Greeting(bool),
    Card,
    Persona,
    Session,
    Branch,
    Enable,
    Disable,
    Uninstall,
}
pub(super) struct Field {
    pub name: &'static str,
    pub editor: TextArea<'static>,
    pub secret: bool,
}
#[derive(Clone, Copy)]
pub(super) enum Hit {
    Transcript,
    Step(usize),
    Tab(usize),
    Item(usize),
    Slash(&'static str),
    Apply,
    Close,
}
pub(super) enum Overlay {
    Menu {
        title: String,
        items: Vec<(String, Choice)>,
        selected: usize,
        filter: String,
        /// Each tab has a label and the item indexes that it shows. An empty list shows all items.
        tabs: Vec<(String, Vec<usize>)>,
        tab: usize,
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
    /// The preview shows a character before the import.
    Preview {
        card: Box<rustytavern::card::Card>,
        image: Option<Box<ratatui_image::protocol::StatefulProtocol>>,
        import: Box<Command>,
        tab: usize,
        scroll: u16,
    },
}
/// A step is a thinking trace or a tool call that comes before the reply. `tool` is absent for thinking.
pub(super) struct Step {
    pub tool: Option<String>,
    pub body: String,
    pub done: bool,
    pub open: bool,
}
pub(super) struct Message {
    pub user: bool,
    pub label: String,
    pub text: String,
}
pub(super) struct Ui {
    pub path: PathBuf,
    /// The enabled extensions set the user and character bubble colors.
    pub bubbles: [ratatui::style::Color; 2],
    pub preferences: Preferences,
    pub composer: TextArea<'static>,
    pub overlay: Option<Overlay>,
    pub transcript: Vec<Message>,
    pub outgoing: String,
    pub names: [String; 2],
    pub reply: String,
    pub title: String,
    pub notice: String,
    pub busy: bool,
    pub exit: bool,
    pub scroll: u16,
    pub tick: usize,
    pub pending: Option<Job>,
    pub new_requested: bool,
    pub hits: Vec<(Rect, Hit)>,
    pub slash: usize,
    pub streaming: bool,
    pub steps: Vec<Step>,
    /// The model and provider of the open chat, or of new chats.
    pub model: Option<[String; 2]>,
    pub failure: Option<String>,
    /// The anchor and the end of a mouse selection in screen cells.
    pub selection: Option<(Position, Position)>,
    pub copy: bool,
    pub copied: Option<String>,
    /// The picker selects the image protocol of the terminal.
    pub picker: ratatui_image::picker::Picker,
    /// The file browser opens this folder.
    pub folder: PathBuf,
}

pub(super) fn slash_matches(
    prefix: &str,
) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    COMMANDS
        .iter()
        .filter(move |(name, _)| name.starts_with(prefix))
}

/// The function returns the item indexes whose labels contain the filter.
pub(super) fn visible(
    items: &[(String, Choice)],
    filter: &str,
    tabs: &[(String, Vec<usize>)],
    tab: usize,
) -> Vec<usize> {
    let filter = filter.to_lowercase();
    let all: Vec<usize> = (0..items.len()).collect();
    let members = tabs.get(tab).map_or(&all, |(_, members)| members);
    members
        .iter()
        .copied()
        .filter(|&index| items[index].0.to_lowercase().contains(&filter))
        .collect()
}

/// Each pair recommends the newest catalog model whose ID starts with the prefix and contains the part.
const RECOMMENDED: &[(&str, &str)] = &[
    ("anthropic/claude-sonnet", ""),
    ("deepseek/deepseek-chat", ""),
    ("google/gemini-", "-pro"),
    ("moonshotai/kimi-k2", ""),
    ("z-ai/glm-", ""),
    ("x-ai/grok-4", ""),
    ("mistralai/mistral-nemo", ""),
];

/// The function returns the release day as days since 1970-01-01. Catalogs supply `created` seconds or a `created_at` date.
fn released(model: &rustytavern::provider::catalog::ModelMetadata) -> Option<i64> {
    if let Some(seconds) = model.raw["created"].as_i64() {
        return Some(seconds.div_euclid(86_400));
    }
    let date = model.raw["created_at"].as_str()?.get(..10)?;
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let year = y - era * 400;
    let day = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    Some(era * 146_097 + (year * 365 + year / 4 - year / 100 + day) - 719_468)
}

fn date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day = z - era * 146_097;
    let year = (day - day / 1460 + day / 36_524 - day / 146_096) / 365;
    let in_year = day - (365 * year + year / 4 - year / 100);
    let month = (5 * in_year + 2) / 153;
    let d = in_year - (153 * month + 2) / 5 + 1;
    let m = if month < 10 { month + 3 } else { month - 9 };
    format!("{}-{m:02}-{d:02}", year + era * 400 + i64::from(m <= 2))
}

fn model_label(model: &rustytavern::provider::catalog::ModelMetadata, recommended: bool) -> String {
    let advertised = &model.advertised;
    let mut parts = vec![format!(
        "{}{}",
        if recommended { "★ " } else { "  " },
        text(&model.id)
    )];
    if let Some(days) = released(model) {
        parts.push(date(days));
    }
    if let Some(length) = advertised.context_length {
        parts.push(format!("{}K context", length / 1000));
    }
    let price = |key: &str| {
        advertised.pricing.as_ref()?[key]
            .as_str()?
            .parse::<f64>()
            .ok()
            .filter(|value| *value >= 0.0)
    };
    match (price("prompt"), price("completion")) {
        (Some(0.0), Some(0.0)) => parts.push("free".into()),
        (Some(input), Some(output)) => parts.push(format!(
            "${:.2} in, ${:.2} out per 1M tokens",
            input * 1e6,
            output * 1e6
        )),
        _ => {}
    }
    parts.join(" · ")
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
fn text_label(value: &str) -> String {
    text(value).replace('\n', " ")
}
fn editor(value: &str) -> TextArea<'static> {
    TextArea::from(text(value).lines().map(str::to_owned).collect::<Vec<_>>())
}
pub(super) fn field(name: &'static str, value: &str) -> Field {
    Field {
        name,
        editor: editor(value),
        secret: false,
    }
}

impl Ui {
    pub fn open(path: PathBuf) -> Result<Self> {
        let app = Application::open(&path)?;
        let (preferences, notice) = match app.store.state_read(PREFERENCES.0, PREFERENCES.1)? {
            Some(value) => match serde_json::from_value(value) {
                Ok(value) => (value, String::new()),
                Err(_) => (
                    Preferences::default(),
                    "The terminal preferences are invalid. Use /chats to select a chat.".into(),
                ),
            },
            None => (
                Preferences::default(),
                "Use /provider, /character, /persona, then /new.".into(),
            ),
        };
        let mut preferences: Preferences = preferences;
        preferences.provider = app.config.default_provider.clone();
        let theme = rustytavern::extension::theme(&app.config.extensions);
        let mut ui = Self {
            bubbles: super::view::bubbles(&theme),
            path,
            preferences,
            composer: editor(""),
            overlay: None,
            transcript: Vec::new(),
            outgoing: String::new(),
            names: ["Character".into(), "You".into()],
            reply: String::new(),
            title: "RustyTavern".into(),
            notice,
            busy: false,
            exit: false,
            scroll: 0,
            tick: 0,
            pending: None,
            new_requested: false,
            hits: Vec::new(),
            slash: 0,
            streaming: false,
            model: None,
            steps: Vec::new(),
            failure: None,
            selection: None,
            copy: false,
            copied: None,
            picker: ratatui_image::picker::Picker::from_fontsize((10, 20)),
            folder: std::env::var_os("HOME").map_or_else(
                || std::env::current_dir().unwrap_or_default(),
                |home| {
                    let home = PathBuf::from(home);
                    let downloads = home.join("Downloads");
                    if downloads.is_dir() { downloads } else { home }
                },
            ),
        };
        if let Err(error) = ui.refresh() {
            ui.notice = error.to_string();
            ui.preferences.session = None;
        }
        Ok(ui)
    }
    /// The method saves the terminal selections in the database.
    pub fn save(&self) -> Result<()> {
        let mut app = Application::open(&self.path)?;
        app.store.state_write(
            PREFERENCES.0,
            PREFERENCES.1,
            &serde_json::to_value(&self.preferences)?,
        )?;
        if app.config.default_provider != self.preferences.provider {
            app.config.default_provider = self.preferences.provider.clone();
            app.save_config()?;
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<()> {
        let app = Application::open(&self.path)?;
        let Some(id) = &self.preferences.session else {
            self.model = self.preferences.provider.as_ref().and_then(|name| {
                Some([
                    app.config.providers.get(name)?.settings.model.clone(),
                    name.clone(),
                ])
            });
            return Ok(());
        };
        let session = app.store.session(id)?;
        let card = app.store.card(&session.character_id)?;
        let persona = app.store.persona(&session.persona_id)?;
        self.title = format!("{} · {}", card.name(), persona.name);
        self.model = Some([session.model.clone(), session.provider.clone()]);
        self.names = [text_label(card.name()), text_label(&persona.name)];
        let history = app.store.history(id)?;
        let mut budget = 128 * 1024;
        self.transcript = history
            .iter()
            .rev()
            .take(100)
            .map_while(|message| {
                let text = text(&message.text);
                budget = usize::checked_sub(budget, text.len())?;
                let user = message.role == rustytavern::session::Role::User;
                let label = match message.role {
                    rustytavern::session::Role::User => &persona.name,
                    rustytavern::session::Role::Assistant => card.name(),
                    _ => "System",
                };
                Some(Message {
                    user,
                    label: text_label(label),
                    text,
                })
            })
            .collect();
        self.transcript.reverse();
        if self.transcript.len() < history.len() {
            self.transcript.insert(
                0,
                Message {
                    user: false,
                    label: "RustyTavern".into(),
                    text: "The display shows recent messages. The export retains all history."
                        .into(),
                },
            );
        }
        self.outgoing.clear();
        self.scroll = 0;
        Ok(())
    }
    /// The method returns the composer text while it is a partial slash command.
    pub(super) fn typed_command(&self) -> Option<String> {
        let typed = self.composer.lines().join("\n");
        (self.overlay.is_none() && typed.starts_with('/') && !typed.contains(['\n', ' ']))
            .then_some(typed)
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
                mut filter,
                tabs,
                mut tab,
            }) => {
                match key.code {
                    KeyCode::Left | KeyCode::Right if !tabs.is_empty() => {
                        let step = if key.code == KeyCode::Left {
                            tabs.len() - 1
                        } else {
                            1
                        };
                        tab = (tab + step) % tabs.len();
                        selected = 0;
                    }
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected += 1,
                    KeyCode::Backspace => _ = filter.pop(),
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        filter.push(character);
                        selected = 0;
                    }
                    KeyCode::Enter => {
                        if let Some(&index) = visible(&items, &filter, &tabs, tab).get(selected) {
                            let (_, choice) = items.remove(index);
                            if let Err(error) = self.choose(choice) {
                                self.notice = error.to_string();
                            }
                            return Ok(());
                        }
                    }
                    _ => {}
                }
                selected =
                    selected.min(visible(&items, &filter, &tabs, tab).len().saturating_sub(1));
                self.overlay = Some(Overlay::Menu {
                    title,
                    items,
                    selected,
                    filter,
                    tabs,
                    tab,
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
            Some(Overlay::Preview {
                card,
                image,
                import,
                mut tab,
                mut scroll,
            }) => {
                match key.code {
                    KeyCode::Enter => {
                        self.command(*import, After::Card);
                        return Ok(());
                    }
                    KeyCode::Left => tab = (tab + PREVIEW_TABS.len() - 1) % PREVIEW_TABS.len(),
                    KeyCode::Right | KeyCode::Tab => tab = (tab + 1) % PREVIEW_TABS.len(),
                    KeyCode::Down => scroll = scroll.saturating_add(1),
                    KeyCode::Up => scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown => scroll = scroll.saturating_add(8),
                    KeyCode::PageUp => scroll = scroll.saturating_sub(8),
                    _ => {}
                }
                if !matches!(
                    key.code,
                    KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
                ) {
                    scroll = 0;
                }
                self.overlay = Some(Overlay::Preview {
                    card,
                    image,
                    import,
                    tab,
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
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let open = !self.steps.iter().any(|step| step.open);
                    self.steps.iter_mut().for_each(|step| step.open = open);
                }
                KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                KeyCode::Up | KeyCode::Down | KeyCode::Tab if self.typed_command().is_some() => {
                    let typed = self.typed_command().unwrap_or_default();
                    let count = slash_matches(&typed).count();
                    match key.code {
                        KeyCode::Up => self.slash = self.slash.saturating_sub(1),
                        KeyCode::Down => self.slash = (self.slash + 1).min(count.saturating_sub(1)),
                        _ => {
                            if let Some((name, _)) = slash_matches(&typed).nth(self.slash) {
                                self.composer = editor(name);
                                self.composer.move_cursor(tui_textarea::CursorMove::End);
                                self.slash = 0;
                            }
                        }
                    }
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
                        let typed = value.trim();
                        let name = slash_matches(typed)
                            .nth(self.slash)
                            .map_or(typed, |(name, _)| *name);
                        self.slash = 0;
                        if let Err(error) = self.slash(name) {
                            self.notice = error.to_string();
                        }
                    } else if !value.trim().is_empty() {
                        if let Some(session) = self.preferences.session.clone() {
                            self.outgoing = text(&value);
                            self.streaming = true;
                            self.failure = None;
                            self.steps.clear();
                            self.pending = Some(Job::Generate {
                                session,
                                message: value,
                                retry: false,
                            });
                            self.composer = editor("");
                            self.reply.clear();
                            self.notice = "The model is replying. Ctrl+C cancels.".into();
                        } else {
                            self.notice = "Select a chat with /new or /chats.".into();
                        }
                    }
                }
                _ => {
                    if self.composer.lines().iter().map(String::len).sum::<usize>() < 128 * 1024 {
                        self.composer.input(key);
                    }
                    self.slash = 0;
                }
            },
        }
        Ok(())
    }
    /// The wheel scrolls. A pointer over a list item selects it. A left click activates the target.
    pub fn mouse(&mut self, event: MouseEvent) -> Result<()> {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(Position::new(event.column, event.row)))
            .map(|(_, hit)| *hit);
        let down = event.kind == MouseEventKind::ScrollDown;
        let position = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.overlay.is_none() => {
                if let Some(Hit::Step(index)) = hit {
                    if let Some(step) = self.steps.get_mut(index) {
                        step.open = !step.open;
                    }
                    return Ok(());
                }
                self.selection = (self.overlay.is_none() && matches!(hit, Some(Hit::Transcript)))
                    .then_some((position, position));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((_, end)) = &mut self.selection {
                    *end = position;
                }
                return Ok(());
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.copy = self.selection.is_some_and(|(start, end)| start != end);
                return Ok(());
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => self.selection = None,
            _ => {}
        }
        match (event.kind, &mut self.overlay) {
            (MouseEventKind::ScrollUp | MouseEventKind::ScrollDown, overlay) => match overlay {
                Some(Overlay::Menu {
                    items,
                    selected,
                    filter,
                    tabs,
                    tab,
                    ..
                }) => {
                    let last = visible(items, filter, tabs, *tab).len().saturating_sub(1);
                    *selected = if down {
                        (*selected + 1).min(last)
                    } else {
                        selected.saturating_sub(1)
                    };
                }
                Some(Overlay::Document { scroll, .. } | Overlay::Preview { scroll, .. })
                    if down =>
                {
                    *scroll = scroll.saturating_add(3)
                }
                Some(Overlay::Document { scroll, .. } | Overlay::Preview { scroll, .. }) => {
                    *scroll = scroll.saturating_sub(3)
                }
                Some(Overlay::Form { .. }) => {}
                None if down => self.scroll = self.scroll.saturating_sub(3),
                None => self.scroll = self.scroll.saturating_add(3),
            },
            _ if self.busy => {}
            (MouseEventKind::Moved, Some(Overlay::Menu { selected, .. })) => {
                if let Some(Hit::Item(index)) = hit {
                    *selected = index;
                }
            }
            (MouseEventKind::Down(MouseButton::Left), overlay) => match (hit, overlay) {
                (Some(Hit::Close), _) => self.key(KeyCode::Esc.into())?,
                (Some(Hit::Slash(name)), _) => {
                    self.composer = editor("");
                    self.slash(name)?;
                }
                (Some(Hit::Apply), _) => self.key(KeyCode::Enter.into())?,
                (Some(Hit::Tab(index)), Some(Overlay::Menu { selected, tab, .. })) => {
                    *tab = index;
                    *selected = 0;
                }
                (Some(Hit::Tab(index)), Some(Overlay::Preview { tab, scroll, .. })) => {
                    *tab = index;
                    *scroll = 0;
                }
                (Some(Hit::Item(index)), Some(Overlay::Menu { selected, .. })) => {
                    *selected = index;
                    self.key(KeyCode::Enter.into())?;
                }
                (Some(Hit::Item(index)), Some(Overlay::Form { selected, .. })) => *selected = index,
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }
    pub(super) fn menu(&mut self, title: &str, items: Vec<(String, Choice)>) {
        self.tabbed_menu(title, items, Vec::new());
    }
    pub(super) fn tabbed_menu(
        &mut self,
        title: &str,
        items: Vec<(String, Choice)>,
        tabs: Vec<(String, Vec<usize>)>,
    ) {
        self.overlay = Some(Overlay::Menu {
            title: title.into(),
            items,
            selected: 0,
            filter: String::new(),
            tabs,
            tab: 0,
        });
    }
    pub(super) fn document(&mut self, title: &str, value: &Value) {
        self.overlay = Some(Overlay::Document {
            title: title.into(),
            text: text(&serde_json::to_string_pretty(value).unwrap_or_default()),
            scroll: 0,
        });
    }
    pub(super) fn catalogs(&mut self, providers: Vec<String>) {
        self.pending = Some(Job::Catalogs(providers));
        self.notice = "The model lists are loading.".into();
    }
    pub(super) fn command(&mut self, command: Command, after: After) {
        self.pending = Some(Job::Command(command, after));
        self.notice = "The application is applying the operation.".into();
    }
    pub(super) fn active(&self) -> Result<String> {
        self.preferences
            .session
            .clone()
            .ok_or_else(|| Error::new("invalid_session", "Select a chat with /new or /chats."))
    }
    pub fn complete(&mut self, result: Result<Vec<Value>>, after: After) {
        let result = (|| -> Result<()> {
            let values = result?;
            let value = values.last().cloned().unwrap_or(Value::Null);
            match after {
                After::Models(providers) => {
                    let mut failures = Vec::new();
                    let mut models = Vec::new();
                    for (provider, value) in providers.iter().zip(values) {
                        if let Some(error) = value["error"].as_str() {
                            failures.push(format!("{provider}: {error}"));
                            continue;
                        }
                        let catalog: rustytavern::provider::catalog::ModelCatalog =
                            serde_json::from_value(value)?;
                        let picks: Vec<String> = RECOMMENDED
                            .iter()
                            .filter_map(|(prefix, part)| {
                                catalog.models.iter().find(|model| {
                                    model.id.starts_with(prefix)
                                        && model.id.contains(part)
                                        && !model.id.contains(':')
                                })
                            })
                            .map(|model| model.id.clone())
                            .collect();
                        for model in catalog.models {
                            let rank = picks.iter().position(|pick| *pick == model.id);
                            models.push((rank, provider.clone(), model));
                        }
                    }
                    models.sort_by_key(|(_, _, model)| std::cmp::Reverse(released(model)));
                    let width = providers.iter().map(String::len).max().unwrap_or(0);
                    let mut tabs = vec![
                        ("★ Recommended".to_owned(), Vec::new()),
                        ("All".to_owned(), (0..models.len()).collect()),
                    ];
                    for provider in &providers {
                        tabs.push((provider.clone(), Vec::new()));
                    }
                    let mut items = Vec::new();
                    for (index, (rank, provider, model)) in models.into_iter().enumerate() {
                        if rank.is_some() {
                            tabs[0].1.push(index);
                        }
                        if let Some(tab) =
                            tabs.iter_mut().skip(2).find(|(name, _)| *name == provider)
                        {
                            tab.1.push(index);
                        }
                        items.push((
                            format!(
                                "{:width$} │ {}",
                                text(&provider),
                                model_label(&model, rank.is_some())
                            ),
                            Choice::Select(Selection::DiscoveredModel(provider), model.id),
                        ));
                    }
                    for tab in &mut tabs {
                        tab.0 = format!("{} ({})", tab.0, tab.1.len());
                    }
                    if tabs[0].1.is_empty() {
                        tabs.remove(0);
                    }
                    self.tabbed_menu(
                        "Models · newest first · ←→ changes the tab · type to filter",
                        items,
                        tabs,
                    );
                    self.notice = if failures.is_empty() {
                        "The catalog advertises capabilities. The catalog does not verify them."
                            .into()
                    } else {
                        format!(
                            "Some catalogs failed. {} Use /settings to type a model ID.",
                            failures.join(" ")
                        )
                    };
                    return Ok(());
                }
                After::Setup(name) => {
                    self.catalogs(vec![name]);
                    return Ok(());
                }
                After::Card => self.preferences.character = value["id"].as_str().map(str::to_owned),
                After::Preview(address) => {
                    let [url, source] = *address;
                    let card = serde_json::from_value(value)?;
                    let import = Command::Card {
                        command: CardCommand::Fetch {
                            url,
                            source,
                            json: true,
                            preview: false,
                        },
                    };
                    self.preview(card, import);
                    return Ok(());
                }
                After::Persona => {
                    self.preferences.persona = value["id"].as_str().map(str::to_owned)
                }
                After::Provider(name) => {
                    self.preferences.provider = Some(name);
                    self.save()?;
                    if self.preferences.session.is_some() && !self.new_requested {
                        self.slash("/adopt")?;
                        self.notice = "New chats use this model. Apply it to this chat too?".into();
                        return Ok(());
                    }
                }
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
    /// The method renders `text_delta`, `reasoning_delta`, `tool_call`, and `tool_result` events.
    pub fn event(&mut self, event: RuntimeEvent) {
        let value = event.text.as_deref().map(text).unwrap_or_default();
        let thinking = |step: &Step| step.tool.is_none();
        match event.r#type.as_str() {
            "text_delta" if self.reply.len() < 128 * 1024 => {
                self.steps
                    .iter_mut()
                    .filter(|s| thinking(s))
                    .for_each(|s| s.done = true);
                self.reply.push_str(&value);
            }
            "reasoning_delta" => match self.steps.last_mut() {
                Some(step) if thinking(step) && !step.done => {
                    if step.body.len() < 128 * 1024 {
                        step.body.push_str(&value);
                    }
                }
                _ => self.steps.push(Step {
                    tool: None,
                    body: value,
                    done: false,
                    open: false,
                }),
            },
            "tool_call" => {
                self.steps
                    .iter_mut()
                    .filter(|s| thinking(s))
                    .for_each(|s| s.done = true);
                self.steps.push(Step {
                    tool: Some(value),
                    body: String::new(),
                    done: false,
                    open: false,
                });
            }
            "tool_result" => {
                if let Some(step) = self
                    .steps
                    .iter_mut()
                    .rev()
                    .find(|s| !thinking(s) && !s.done)
                {
                    step.body = value;
                    step.done = true;
                }
            }
            _ => {}
        }
    }
    pub fn finish(&mut self, result: Result<Generation>) {
        self.streaming = false;
        self.steps.iter_mut().for_each(|step| step.done = true);
        let unsent = std::mem::take(&mut self.outgoing);
        self.reply.clear();
        let _ = self.refresh();
        match result {
            Ok(generation) if generation.status == GenerationStatus::Completed => {
                self.notice = "The reply is saved.".into();
            }
            Ok(generation) => {
                self.reply = text(&generation.draft);
                self.failure = Some(generation.error.map_or_else(
                    || "The generation ended.".into(),
                    |error| text(&error.message),
                ));
                self.notice = "The draft remains saved. Use /retry to try again.".into();
            }
            Err(error)
                if matches!(
                    &*error.code,
                    "pinned_composition_unavailable" | "session_configuration_changed"
                ) =>
            {
                self.composer = editor(&unsent);
                let _ = self.slash("/adopt");
                self.notice = "The setup changed after this conversation started. Apply the current settings, then press Enter to send again.".into();
            }
            Err(error) => {
                self.failure = Some(text(&error.message));
                self.notice = error.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests;
