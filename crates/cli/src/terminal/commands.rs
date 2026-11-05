use super::state::*;
use super::*;

impl Ui {
    pub(super) fn slash(&mut self, name: &str) -> Result<()> {
        let app = Application::open(&self.path)?;
        match name {
            "/models" => {
                let mut items: Vec<_> = app
                    .config
                    .providers
                    .keys()
                    .map(|name| (text(name), Choice::Form(FormKind::Models(name.clone()))))
                    .collect();
                items.push(("Add a provider...".into(), Choice::Form(FormKind::Provider)));
                self.menu("Discover models from a provider", items);
            }
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
                        "{}\n\nEnter sends. Alt+Enter inserts a newline.\nTab completes commands or changes fields. Escape closes dialogs.\nPage Up and Page Down scroll. Ctrl+C cancels. Ctrl+D exits.",
                        COMMANDS
                            .iter()
                            .map(|(a, b)| format!("{a}  {b}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    scroll: 0,
                })
            }
            "/setup" => self.form(FormKind::Provider),
            "/character" | "/persona" | "/sessions" => {
                let (kind, title) = match name {
                    "/character" => ("card", "Characters"),
                    "/persona" => ("persona", "Personas"),
                    _ => ("session", "Conversations"),
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
                        "Import a character...".into(),
                        Choice::Form(FormKind::Import),
                    ));
                    items.push((
                        "Fetch a website character...".into(),
                        Choice::Form(FormKind::Fetch),
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
            "/model" => {
                let mut items = app
                    .config
                    .providers
                    .iter()
                    .map(|(name, p)| {
                        (
                            format!("{name}  {}", p.model),
                            Choice::Select(Selection::Provider, name.clone()),
                        )
                    })
                    .collect::<Vec<_>>();
                items.push((
                    "Add or edit a provider...".into(),
                    Choice::Form(FormKind::Provider),
                ));
                self.menu("Model profiles", items);
            }
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
                    "Adopt selected provider and current plugins?",
                    vec![(
                        "Apply current settings".into(),
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
                Selection::Provider => {
                    self.preferences.provider = Some(id);
                    self.save()?;
                    self.notice = "The model selection applies to /new or /adopt.".into();
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
            self.form(FormKind::New);
        }
        Ok(())
    }
}
