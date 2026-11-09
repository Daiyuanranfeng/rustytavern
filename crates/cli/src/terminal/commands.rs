use super::state::*;
use super::*;
use rustytavern::session::{GenerationMode, Role};

impl Ui {
    pub(super) fn slash(&mut self, name: &str) -> Result<()> {
        let app = Application::open(&self.path)?;
        let name = match name {
            "/setup" => "/provider",
            "/sessions" => "/chats",
            "/models" => "/model",
            "/branch" => "/messages",
            other => other,
        };
        match name {
            "/worldbook" => {
                let target = self
                    .preferences
                    .session
                    .as_deref()
                    .map(|id| app.store.session(id))
                    .transpose()?;
                let mut items = vec![
                    (
                        "Import a worldbook...".into(),
                        Choice::Form(FormKind::WorldbookImport),
                    ),
                    (
                        "Export a worldbook...".into(),
                        Choice::Form(FormKind::WorldbookExport),
                    ),
                ];
                for record in app.store.list("worldbook")? {
                    let id = record["id"]
                        .as_str()
                        .ok_or_else(|| {
                            Error::new("invalid_worldbook_archive", "The worldbook has no ID.")
                        })?
                        .to_owned();
                    let document = &record["document"];
                    let name = document["name"]
                        .as_str()
                        .or(document["data"]["name"].as_str())
                        .unwrap_or(&id)
                        .to_owned();
                    let name = text(&name);
                    if let Some(target) = &target {
                        let mut books = target.worldbook_ids.clone();
                        let selected = books.contains(&id);
                        if selected {
                            books.retain(|book| book != &id);
                        } else {
                            books.push(id);
                        }
                        items.push((
                            format!("{} {name}", if selected { "Remove" } else { "Add" }),
                            Choice::Command(
                                Command::Worldbook {
                                    command: WorldbookCommand::Bind {
                                        session: target.id.clone(),
                                        books,
                                        expected_version: Some(target.version),
                                    },
                                },
                                After::Refresh,
                            ),
                        ));
                    } else {
                        items.push((
                            name,
                            Choice::Command(
                                Command::Worldbook {
                                    command: WorldbookCommand::Inspect { worldbook: id },
                                },
                                After::View("Worldbook"),
                            ),
                        ));
                    }
                }
                self.menu("Worldbooks for this conversation", items);
            }
            "/quit" => self.exit = true,
            "/verbose" => {
                self.preferences.verbose = !self.preferences.verbose;
                self.save()?;
                let state = if self.preferences.verbose {
                    "on"
                } else {
                    "off"
                };
                self.notice = format!("Verbose mode is {state}.");
            }
            "/help" => {
                self.overlay = Some(Overlay::Document {
                    title: "Commands and keys".into(),
                    text: format!(
                        "{}\n\nEnter sends. Alt+Enter inserts a newline.\nTab completes commands or changes fields. Escape closes dialogs.\nThe chat history stays in the terminal scrollback. Scroll and select it as usual.\nDialogs accept the mouse wheel and clicks.\nCtrl+O or /verbose turns verbose mode on or off. Verbose mode shows the thinking text, the tool calls, and the web search sources.\nCtrl+C cancels. Ctrl+D exits.\n\nThese keys work while the message box is empty:\n← and → show the alternatives of the last message. → after the newest reply generates another reply.\n↑ edits the last message. Ctrl+R regenerates the last reply.\nEsc twice lists the messages of this branch. Select a message to rewind, edit, regenerate, branch, hide, or copy it.",
                        COMMANDS
                            .iter()
                            .map(|(a, b)| format!("{a}  {b}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    scroll: 0,
                })
            }
            "/provider" => {
                let providers = &app.config.providers;
                let mut items: Vec<_> = super::forms::VENDORS
                    .iter()
                    .map(|(id, _, label, _)| {
                        let status = if providers.contains_key(*id) {
                            "✓ connected · select to replace the key"
                        } else {
                            "not connected"
                        };
                        (
                            format!("{label} · {status}"),
                            Choice::Form(FormKind::Setup((*id).into())),
                        )
                    })
                    .collect();
                items.extend(
                    providers
                        .keys()
                        .filter(|name| !super::forms::VENDORS.iter().any(|v| v.0 == *name))
                        .map(|name| {
                            (
                                format!("{} · ✓ connected · select to edit", text(name)),
                                Choice::Form(FormKind::Provider(name.clone())),
                            )
                        }),
                );
                items.push((
                    "Local or custom server · OpenAI-compatible, for example Ollama or LM Studio"
                        .into(),
                    Choice::Form(FormKind::Setup("custom".into())),
                ));
                self.menu(
                    &format!("Model providers · {} connected", providers.len()),
                    items,
                );
            }
            "/character" | "/persona" | "/chats" => {
                let (kind, title) = match name {
                    "/character" => ("card", "Characters"),
                    "/persona" => ("persona", "Personas"),
                    _ => ("session", "Chats"),
                };
                let mut items = Vec::new();
                for record in app.store.list(kind)? {
                    let id = record["id"].as_str().unwrap_or_default().to_owned();
                    let label = if kind == "card" {
                        record["document"]["data"]["name"]
                            .as_str()
                            .unwrap_or(&id)
                            .to_owned()
                    } else if kind == "persona" {
                        record["name"].as_str().unwrap_or(&id).to_owned()
                    } else {
                        let title = record["title"].as_str().unwrap_or(&id);
                        let active = self.preferences.session.as_deref() == Some(id.as_str());
                        format!(
                            "{}{title}  {}",
                            if active { "● " } else { "" },
                            record["model"].as_str().unwrap_or_default()
                        )
                    };
                    items.push((
                        text(&label),
                        Choice::Select(
                            match kind {
                                "card" => Selection::Card,
                                "persona" => Selection::Persona,
                                _ => Selection::Session,
                            },
                            id,
                        ),
                    ));
                }
                if kind == "card" {
                    items.push((
                        "Add a character from a file...".into(),
                        Choice::Browse(self.folder.clone()),
                    ));
                    items.push((
                        "Add a character from a web address...".into(),
                        Choice::Form(FormKind::Import),
                    ));
                }
                if kind == "persona" {
                    items.push((
                        "Create a persona...".into(),
                        Choice::Form(FormKind::Persona),
                    ));
                }
                if let Some(session) = self
                    .preferences
                    .session
                    .clone()
                    .filter(|_| kind == "session")
                {
                    items.push(("Rename this chat...".into(), Choice::Form(FormKind::Rename)));
                    items.push((
                        "Delete this chat...".into(),
                        Choice::Select(Selection::Delete, session),
                    ));
                }
                self.menu(title, items);
            }
            "/model" if app.config.providers.is_empty() => self.slash("/provider")?,
            "/model" => self.catalogs(app.config.providers.keys().cloned().collect()),
            "/settings" => self.menu(
                "Settings",
                [
                    ("Lorebooks for this chat", "/worldbook"),
                    ("Messages and branches", "/messages"),
                    ("Export this chat", "/export"),
                    ("Inspect the next model request", "/prompt"),
                    (
                        "Apply the current provider and plugins to this chat",
                        "/adopt",
                    ),
                    ("Edit provider settings (advanced)", "/profile"),
                    ("Plugins", "/plugins"),
                    ("Show the configuration", "/config"),
                ]
                .into_iter()
                .map(|(label, name)| (label.into(), Choice::Slash(name)))
                .collect(),
            ),
            "/profile" => self.form(FormKind::Provider(
                self.preferences
                    .provider
                    .clone()
                    .unwrap_or_else(|| "default".into()),
            )),
            "/new" => {
                self.new_requested = true;
                self.advance_new()?;
            }
            "/config" => self.command(
                Command::Config {
                    command: ConfigCommand::Show,
                },
                After::View("Configuration"),
            ),
            "/plugins" => {
                let mut items = vec![
                    (
                        "Inspect plugins".into(),
                        Choice::Command(
                            Command::Plugin {
                                command: PluginCommand::List { json: true },
                            },
                            After::View("Plugins"),
                        ),
                    ),
                    (
                        "Install a plugin...".into(),
                        Choice::Form(FormKind::Install),
                    ),
                    (
                        "Run a plugin command...".into(),
                        Choice::Form(FormKind::Run),
                    ),
                    ("Bind a service...".into(), Choice::Form(FormKind::Bind)),
                    (
                        "Inspect composition".into(),
                        Choice::Command(
                            Command::Preset {
                                command: PresetCommand::Inspect {
                                    preset: "tavern".into(),
                                    resolved: true,
                                    json: true,
                                },
                            },
                            After::View("Composition"),
                        ),
                    ),
                ];
                let registry = rustytavern::plugin::PluginRegistry::default();
                let manager = PluginManager::open(
                    app.store.clone(),
                    std::sync::Arc::new(tokio::sync::Mutex::new(registry)),
                    &app.config.plugins,
                );
                for installed in manager.list()? {
                    let id = installed["manifest"]["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    items.push((
                        format!("Enable {id}"),
                        Choice::Select(Selection::Enable, id.clone()),
                    ));
                    items.push((
                        format!("Disable {id}"),
                        Choice::Select(Selection::Disable, id.clone()),
                    ));
                    items.push((
                        format!("Uninstall {id}"),
                        Choice::Select(Selection::Uninstall, id),
                    ));
                }
                self.menu("Plugins", items);
            }
            "/prompt" => {
                self.active()?;
                self.form(FormKind::Prompt);
            }
            "/export" => {
                self.active()?;
                self.form(FormKind::Export);
            }
            "/adopt" => {
                let session = self.active()?;
                self.menu(
                    "Adopt the selected provider and current plugins for this conversation?",
                    vec![(
                        "Apply the current settings".into(),
                        Choice::Command(
                            Command::Session {
                                command: SessionCommand::Configure {
                                    session,
                                    provider: self.preferences.provider.clone(),
                                    model: None,
                                    context_limit: None,
                                    output_limit: None,
                                    expected_version: None,
                                },
                            },
                            After::Refresh,
                        ),
                    )],
                );
            }
            "/retry" | "/continue" | "/impersonate" => {
                let session = self.active()?;
                self.mode = match name {
                    "/continue" => GenerationMode::Continue,
                    "/impersonate" => GenerationMode::Impersonate,
                    _ => GenerationMode::Reply,
                };
                self.streaming = true;
                self.failure = None;
                self.steps.clear();
                self.pending = Some(Job::Generate {
                    session,
                    message: String::new(),
                    retry: name == "/retry",
                    mode: self.mode,
                });
                self.reply.clear();
            }
            "/messages" => {
                let session = app.store.session(&self.active()?)?;
                let items = app
                    .store
                    .history(&session.id)?
                    .into_iter()
                    .rev()
                    .map(|message| {
                        let name = &self.names[usize::from(message.role == Role::User)];
                        let hidden = if session.hidden.contains(&message.id) {
                            " · hidden"
                        } else {
                            ""
                        };
                        (
                            format!("{name}{hidden} · {}", preview(&message.text)),
                            Choice::Select(Selection::Message, message.id),
                        )
                    })
                    .collect();
                self.menu(
                    "Messages in this branch · newest first · Enter shows the actions",
                    items,
                );
            }
            _ => {
                return Err(Error::new(
                    "invalid_command",
                    "The slash command does not exist. Use /help.",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn choose(&mut self, choice: Choice) -> Result<()> {
        match choice {
            Choice::Slash(name) => self.slash(name)?,
            Choice::Form(kind) => self.form(kind),
            Choice::Command(command, after) => self.command(command, after),
            Choice::Browse(folder) => self.browse(folder)?,
            Choice::Copy(value) => {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(value);
                print!("\x1b]52;c;{encoded}\x07");
                std::io::Write::flush(&mut std::io::stdout())?;
                self.notice =
                    "The terminal copied the text if it allows clipboard access (OSC 52).".into();
            }
            Choice::Open(path) => {
                if let Err(error) = self.open_card(path) {
                    self.browse(self.folder.clone())?;
                    self.notice = error.to_string();
                }
            }
            Choice::Select(kind, id) => match kind {
                Selection::Greeting(reduced) => {
                    self.new_requested = false;
                    let missing =
                        |what: &str| Error::new("invalid_input", format!("Select a {what}."));
                    self.command(
                        Command::Session {
                            command: SessionCommand::New {
                                character: self
                                    .preferences
                                    .character
                                    .clone()
                                    .ok_or_else(|| missing("character"))?,
                                persona: self
                                    .preferences
                                    .persona
                                    .clone()
                                    .ok_or_else(|| missing("persona"))?,
                                provider: self
                                    .preferences
                                    .provider
                                    .clone()
                                    .ok_or_else(|| missing("provider"))?,
                                model: None,
                                greeting: id.parse().unwrap_or_default(),
                                accept_reduced: reduced,
                                json: true,
                            },
                        },
                        After::Session,
                    );
                }
                Selection::DiscoveredModel(provider) => self.command(
                    Command::Model {
                        command: ModelCommand::Select {
                            provider: provider.clone(),
                            model: id,
                        },
                    },
                    After::Provider(provider),
                ),
                Selection::Card => {
                    self.preferences.character = Some(id);
                    self.save()?;
                    self.advance_new()?;
                }
                Selection::Persona => {
                    self.preferences.persona = Some(id);
                    self.save()?;
                    self.advance_new()?;
                }
                Selection::Session => {
                    self.preferences.session = Some(id);
                    self.refresh()?;
                    self.save()?;
                    self.notice =
                        "The conversation is open. Use /adopt if its configuration changed.".into();
                }
                Selection::Branch => self.command(
                    Command::Session {
                        command: SessionCommand::Branch {
                            session: self.active()?,
                            at: id,
                        },
                    },
                    After::Say("The chat continues from the selected message."),
                ),
                Selection::Message => self.actions(&id)?,
                Selection::Alternatives => self.alternatives(&id)?,
                Selection::Delete => self.menu(
                    "Delete this chat and its history? You cannot undo this.",
                    vec![(
                        "Delete the chat".into(),
                        Choice::Command(
                            Command::Session {
                                command: SessionCommand::Delete { session: id },
                            },
                            After::Session,
                        ),
                    )],
                ),
                Selection::Enable => self.command(
                    Command::Plugin {
                        command: PluginCommand::Enable { plugin: id },
                    },
                    After::Notice,
                ),
                Selection::Disable => self.command(
                    Command::Plugin {
                        command: PluginCommand::Disable { plugin: id },
                    },
                    After::Notice,
                ),
                Selection::Uninstall => self.command(
                    Command::Plugin {
                        command: PluginCommand::Uninstall { plugin: id },
                    },
                    After::Notice,
                ),
            },
        }
        Ok(())
    }
    pub(super) fn advance_new(&mut self) -> Result<()> {
        if !self.new_requested {
            return Ok(());
        }
        if self.preferences.provider.is_none() {
            self.slash("/model")?;
        } else if self.preferences.character.is_none() {
            self.slash("/character")?;
        } else if self.preferences.persona.is_none() {
            self.slash("/persona")?;
        } else {
            self.greetings()?;
        }
        Ok(())
    }
    /// The method lists the actions for one message of the open chat.
    fn actions(&mut self, id: &str) -> Result<()> {
        let app = Application::open(&self.path)?;
        let session = app.store.session(&self.active()?)?;
        let messages = app.store.messages(&session.id)?;
        let message = messages
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::new("not_found", "The message does not exist."))?;
        let count = messages
            .iter()
            .filter(|m| m.parent_id == message.parent_id && m.role == message.role)
            .count();
        let hidden = session.hidden.iter().any(|item| item == id);
        let command = |command| Command::Session { command };
        let branch = |at: &str| {
            command(SessionCommand::Branch {
                session: session.id.clone(),
                at: at.to_owned(),
            })
        };
        let mut items = vec![
            (
                "Rewind to here".into(),
                Choice::Command(
                    branch(id),
                    After::Say("The chat continues from the selected message."),
                ),
            ),
            ("Edit...".into(), Choice::Form(FormKind::Edit(id.into()))),
        ];
        if message.role == Role::Assistant
            && let Some(parent) = &message.parent_id
        {
            items.push((
                "Regenerate this reply".into(),
                Choice::Command(branch(parent), After::Retry),
            ));
        }
        if count > 1 {
            items.push((
                format!("Alternatives ({count})..."),
                Choice::Select(Selection::Alternatives, id.into()),
            ));
        }
        items.push((
            "Branch into a new chat".into(),
            Choice::Command(
                command(SessionCommand::Fork {
                    session: session.id.clone(),
                    at: id.into(),
                }),
                After::Session,
            ),
        ));
        items.push((
            if hidden {
                "Include in the prompt"
            } else {
                "Hide from the prompt"
            }
            .into(),
            Choice::Command(
                command(SessionCommand::Hide {
                    session: session.id.clone(),
                    at: id.into(),
                    show: hidden,
                }),
                After::Say(if hidden {
                    "The prompt includes the message."
                } else {
                    "The prompt omits the message. The chat still shows it."
                }),
            ),
        ));
        items.push(("Copy the text".into(), Choice::Copy(message.text.clone())));
        self.menu(&format!("Message · {}", preview(&message.text)), items);
        Ok(())
    }
    /// The method lists the message and its siblings with the same role, oldest first.
    fn alternatives(&mut self, id: &str) -> Result<()> {
        let app = Application::open(&self.path)?;
        let session = self.active()?;
        let messages = app.store.messages(&session)?;
        let message = messages
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::new("not_found", "The message does not exist."))?;
        let items = messages
            .iter()
            .filter(|m| m.parent_id == message.parent_id && m.role == message.role)
            .enumerate()
            .map(|(index, sibling)| {
                let mark = if sibling.id == id { "● " } else { "" };
                (
                    format!("{}. {mark}{}", index + 1, preview(&sibling.text)),
                    Choice::Select(Selection::Branch, sibling.id.clone()),
                )
            })
            .collect();
        self.menu("Alternatives · ● marks the current one", items);
        Ok(())
    }
    /// The method lists the card greetings. A card with unsupported features starts with the reduced profile.
    fn greetings(&mut self) -> Result<()> {
        let app = Application::open(&self.path)?;
        let card = app
            .store
            .card(self.preferences.character.as_deref().unwrap_or_default())?;
        let reduced = card.ensure_compatible(false).is_err();
        let data = card.data();
        let first = data["first_mes"].as_str().unwrap_or_default();
        let alternates = data["alternate_greetings"].as_array().into_iter().flatten();
        let items = std::iter::once(first)
            .chain(alternates.filter_map(|value| value.as_str()))
            .enumerate()
            .map(|(index, greeting)| {
                (
                    format!("{}. {}", index + 1, preview(greeting)),
                    Choice::Select(Selection::Greeting(reduced), index.to_string()),
                )
            })
            .collect();
        self.menu(
            &format!(
                "Choose how {} greets you{}",
                text(card.name()),
                if reduced {
                    " · unsupported card features are ignored"
                } else {
                    ""
                }
            ),
            items,
        );
        Ok(())
    }
}
