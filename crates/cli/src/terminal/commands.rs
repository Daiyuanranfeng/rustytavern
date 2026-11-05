use super::state::*;
use super::*;

impl Ui {
    pub(super) fn slash(&mut self, name: &str) -> Result<()> {
        let app = Application::open(&self.path)?;
        let name = match name {
            "/setup" => "/provider",
            "/sessions" => "/chats",
            "/models" => "/model",
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
            "/help" => {
                self.overlay = Some(Overlay::Document {
                    title: "Commands and keys".into(),
                    text: format!(
                        "{}\n\nEnter sends. Alt+Enter inserts a newline.\nTab completes commands or changes fields. Escape closes dialogs.\nPage Up, Page Down, and the mouse wheel scroll. Click an item to select it.\nHold Shift (Option in some macOS terminals) to select text with the mouse.\nCtrl+C cancels. Ctrl+D exits.",
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
                    .map(|(id, variable, _, label, _)| {
                        let status = if providers.contains_key(*id) {
                            "✓ connected · select to replace the key".into()
                        } else if std::env::var(variable).is_ok_and(|v| !v.trim().is_empty()) {
                            format!("not connected · key found in {variable}")
                        } else {
                            "not connected".into()
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
                        format!("{}  {}", id, record["model"].as_str().unwrap_or_default())
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
                        "Add a character from a file or web address...".into(),
                        Choice::Form(FormKind::Import),
                    ));
                }
                if kind == "persona" {
                    items.push((
                        "Create a persona...".into(),
                        Choice::Form(FormKind::Persona),
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
                    ("Conversation branches", "/branch"),
                    ("Export this chat", "/export"),
                    ("Inspect the next model request", "/prompt"),
                    (
                        "Apply the current provider and extensions to this chat",
                        "/adopt",
                    ),
                    ("Edit provider settings (advanced)", "/profile"),
                    ("Extensions", "/extensions"),
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
            "/plugins" => self.menu(
                "Runtime plugins",
                vec![
                    (
                        "Inspect plugins".into(),
                        Choice::Command(
                            Command::Plugin {
                                command: PluginCommand::Inspect {
                                    plugin: None,
                                    json: true,
                                },
                            },
                            After::View("Runtime plugins"),
                        ),
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
                ],
            ),
            "/extensions" => {
                let mut items = vec![
                    (
                        "Inspect packages".into(),
                        Choice::Command(
                            Command::Extension {
                                command: ExtensionCommand::List { json: true },
                            },
                            After::View("Product extensions"),
                        ),
                    ),
                    (
                        "Install a package...".into(),
                        Choice::Form(FormKind::Install),
                    ),
                    (
                        "Run a package command...".into(),
                        Choice::Form(FormKind::Run),
                    ),
                ];
                let registry = rustytavern::plugin::PluginRegistry::default();
                let manager = ExtensionManager::open(
                    &app.data_dir,
                    app.store.clone(),
                    std::sync::Arc::new(tokio::sync::Mutex::new(registry)),
                )?;
                for package in manager.list() {
                    let id = package.manifest.id;
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
                self.menu("Product extensions", items);
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
                    "Adopt the selected provider and current extensions for this conversation?",
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
            "/retry" => {
                self.streaming = true;
                self.failure = None;
                self.steps.clear();
                self.pending = Some(Job::Generate {
                    session: self.active()?,
                    message: String::new(),
                    retry: true,
                });
                self.reply.clear();
            }
            "/branch" => {
                let mut items = Vec::new();
                for message in app.store.messages(&self.active()?)? {
                    items.push((
                        format!(
                            "{}  {}",
                            message.id,
                            text(&message.text).chars().take(60).collect::<String>()
                        ),
                        Choice::Select(Selection::Branch, message.id),
                    ));
                }
                self.menu("Conversation heads", items);
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
                    After::Refresh,
                ),
                Selection::Enable => self.form(FormKind::Enable(id)),
                Selection::Disable => self.command(
                    Command::Extension {
                        command: ExtensionCommand::Disable { extension: id },
                    },
                    After::Notice,
                ),
                Selection::Uninstall => self.command(
                    Command::Extension {
                        command: ExtensionCommand::Uninstall { extension: id },
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
                let preview: String = text(greeting)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(120)
                    .collect();
                (
                    format!("{}. {preview}", index + 1),
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
