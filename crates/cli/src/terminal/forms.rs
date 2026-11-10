use super::state::*;
use super::*;
use rustytavern::session::Role;

impl Ui {
    pub(super) fn form(&mut self, kind: FormKind) {
        let (title, fields) = match &kind {
            FormKind::Setup(vendor) => {
                let mut fields = vec![Field {
                    secret: true,
                    ..field("API key", "")
                }];
                if vendor == "custom" {
                    fields[0].name = "API key: optional";
                    fields.push(field(
                        "Server URL, for example http://127.0.0.1:8080/v1/chat/completions",
                        "",
                    ));
                }
                ("Connect a model provider", fields)
            }
            FormKind::WorldbookImport => ("Import a worldbook", vec![field("File path", "")]),
            FormKind::Preset => (
                "Import a SillyTavern preset · it replaces the instructions, the prefill, and the blocks",
                vec![field("File path", "")],
            ),
            FormKind::WorldbookExport => (
                "Export a worldbook",
                vec![
                    field("Worldbook ID", ""),
                    field("Output file", "worldbook.json"),
                ],
            ),
            FormKind::Provider(name) => {
                let selected = Application::open(&self.path)
                    .ok()
                    .and_then(|app| app.config.providers.get(name).map(|p| p.settings.clone()));
                let config = selected.unwrap_or_default();
                let kind = match config.kind {
                    rustytavern::provider::ProviderKind::OpenRouter => "openrouter",
                    rustytavern::provider::ProviderKind::OpenAi => "openai",
                    rustytavern::provider::ProviderKind::Anthropic => "anthropic",
                    rustytavern::provider::ProviderKind::Gemini => "gemini",
                    rustytavern::provider::ProviderKind::Custom => "custom",
                };
                (
                    "Model provider",
                    vec![
                        field("Profile name", name),
                        field(
                            "Vendor: openrouter, openai, anthropic, gemini, custom",
                            kind,
                        ),
                        field("Model ID", &config.model),
                        field(
                            "Endpoint: custom providers require a full URL",
                            config.endpoint.as_deref().unwrap_or_default(),
                        ),
                        field("Context limit", &config.context_limit.to_string()),
                        field("Output limit", &config.output_limit.to_string()),
                        field(
                            "Sampling parameters as JSON",
                            &serde_json::to_string(&config.parameters)
                                .unwrap_or_else(|_| "{}".into()),
                        ),
                        field(
                            "OpenRouter routing as JSON: optional",
                            &config
                                .routing
                                .map(|routing| serde_json::to_string(&routing).unwrap_or_default())
                                .unwrap_or_default(),
                        ),
                        field(
                            "Merge system messages: yes or no",
                            if config.merge_system_messages {
                                "yes"
                            } else {
                                "no"
                            },
                        ),
                    ],
                )
            }
            FormKind::Import => (
                "Add a character from the web",
                vec![field(
                    "Paste a chub.ai or realm.risuai.net address. A file path also works.",
                    "",
                )],
            ),
            FormKind::Persona => (
                "Create a persona",
                vec![field("Name", ""), field("Description", "")],
            ),
            FormKind::Export => (
                "Export this conversation",
                vec![
                    field("Output file", "conversation.json"),
                    field("Include request traces: yes or no", "no"),
                ],
            ),
            FormKind::Prompt => (
                "Inspect the next request",
                vec![
                    field("Proposed message", ""),
                    field("Full request output file: optional", ""),
                ],
            ),
            FormKind::Bind => (
                "Bind a plugin service",
                vec![
                    field("Service: profile, provider, driver, decoder, or store", ""),
                    field("Plugin ID", ""),
                ],
            ),
            FormKind::Install => (
                "Install a plugin",
                vec![
                    field("Plugin directory", ""),
                    field("Explicit grants: one per line", ""),
                    field("Trust external processes: yes or no", "no"),
                ],
            ),
            FormKind::Run => (
                "Run a plugin command",
                vec![field("Command name", ""), field("Arguments as JSON", "{}")],
            ),
            FormKind::Edit(id) => {
                let current = self.stored(|app, session| {
                    let message = app
                        .store
                        .messages(session)?
                        .into_iter()
                        .find(|m| m.id == *id);
                    Ok(message.map(|m| m.text))
                });
                (
                    "Edit the message",
                    vec![wrapped("Text · the edit adds a new version", &current)],
                )
            }
            FormKind::Rename => {
                let current = self.stored(|app, session| Ok(app.store.session(session)?.title));
                (
                    "Rename this chat",
                    vec![field("Name · empty removes it", &current)],
                )
            }
            FormKind::Options => {
                let prompt = Application::open(&self.path)
                    .ok()
                    .and_then(|app| app.config.prompt)
                    .unwrap_or_default();
                let number = |name, value: usize| field(name, &value.to_string());
                (
                    "Prompt and memory · new chats and the open chat use these values",
                    vec![
                        number(
                            "History limit: the number of recent messages in the prompt. 0 keeps all that fit.",
                            prompt.history_limit,
                        ),
                        number(
                            "Memory interval: the new messages between two memory updates. 0 turns the memory off.",
                            prompt.memory_interval(),
                        ),
                        number("Context limit in estimated bytes", prompt.context_limit),
                        number("Output limit", prompt.output_limit),
                        number("Lorebook budget in estimated bytes", prompt.lore_budget),
                        number("Example budget in estimated bytes", prompt.example_budget),
                        wrapped(
                            "Main instruction · the system prompt of a card replaces it",
                            &prompt.main_instruction,
                        ),
                        wrapped(
                            "Post-history instruction · other apps call it the jailbreak",
                            &prompt.post_history_instruction,
                        ),
                        field(
                            "Post-history role: system or user. Use user for Gemini.",
                            role(&prompt.post_history_role),
                        ),
                        wrapped("Author's note", &prompt.note),
                        field(
                            "Author's note depth: the messages after it. Empty puts it after your persona.",
                            &prompt.note_depth.map(|d| d.to_string()).unwrap_or_default(),
                        ),
                        field(
                            "Author's note role: system, user, or assistant",
                            role(&prompt.note_role),
                        ),
                        wrapped("Prefill: the start of each reply", &prompt.prefill),
                    ],
                )
            }
            FormKind::Agent => {
                let policy = Application::open(&self.path)
                    .and_then(|app| super::commands::agent_policy(&app.config))
                    .unwrap_or_default();
                let tools = policy.confirm_tools.into_iter().collect::<Vec<_>>();
                (
                    "Agent and tools · new chats and the open chat use these values",
                    vec![
                        field(
                            "Tool steps: the limit of tool rounds for one reply, from 1 to 64",
                            &policy.max_steps.to_string(),
                        ),
                        field(
                            "Tools that wait for your approval: one name on each line. * selects every tool.",
                            &tools.join("\n"),
                        ),
                        field(
                            "Repeats of a cut-off reply: 0 turns them off",
                            &policy.retry_truncated.to_string(),
                        ),
                        field(
                            "Token budget of one reply: 0 turns it off",
                            &policy.max_tokens.to_string(),
                        ),
                    ],
                )
            }
            FormKind::Memory => {
                use rustytavern::runtime::memory::{Memory, visible};
                let memory = self
                    .preferences
                    .session
                    .as_deref()
                    .and_then(|id| {
                        let app = Application::open(&self.path).ok()?;
                        let session = app.store.session(id).ok()?;
                        let history = visible(app.store.as_ref(), &session).ok()?;
                        Some(
                            Memory::load(app.store.as_ref(), &session)
                                .ok()?
                                .current(&history),
                        )
                    })
                    .unwrap_or_default();
                (
                    "Memory of this chat · the prompt includes it before the history",
                    vec![
                        wrapped("Summary of the chat · empty removes it", &memory.summary),
                        wrapped(
                            "Facts about the character and you · one on each line · all chats of this pair share them",
                            &memory.facts.join("\n"),
                        ),
                    ],
                )
            }
        };
        if let FormKind::Setup(vendor) = &kind {
            self.notice = VENDORS.iter().find(|v| v.0 == vendor).map_or_else(
                || "Enter the server URL. A local server often needs no key.".into(),
                |v| format!("Paste a key from {}.", v.3),
            );
        }
        self.overlay = Some(Overlay::Form {
            title: title.into(),
            kind,
            fields,
            selected: 0,
            zoom: false,
        });
    }
    /// The method reads a text of the open chat. A missing chat or text gives an empty text.
    fn stored(&self, read: impl FnOnce(&Application, &str) -> Result<Option<String>>) -> String {
        let Some(session) = &self.preferences.session else {
            return String::new();
        };
        Application::open(&self.path)
            .and_then(|app| read(&app, session))
            .ok()
            .flatten()
            .unwrap_or_default()
    }
    pub(super) fn submit(&mut self, kind: &FormKind, values: &[String]) -> Result<()> {
        let yes = |value: &str| match value.trim() {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(Error::new("invalid_input", "The field requires yes or no.")),
        };
        let number = |value: &str| {
            value
                .trim()
                .parse::<usize>()
                .map_err(|_| Error::new("invalid_input", "The field requires an unsigned number."))
        };
        let (command, after) = match kind {
            FormKind::Preset => (
                Command::Config {
                    command: ConfigCommand::Preset {
                        path: values[0].trim().into(),
                    },
                },
                After::Say("The preset is saved in the prompt settings."),
            ),
            FormKind::WorldbookImport => {
                if values[0] == "-"
                    || !std::fs::metadata(&values[0]).is_ok_and(|metadata| metadata.is_file())
                {
                    return Err(Error::new(
                        "invalid_input",
                        "Select a regular worldbook file.",
                    ));
                }
                (
                    Command::Worldbook {
                        command: WorldbookCommand::Import {
                            path: values[0].clone().into(),
                        },
                    },
                    After::Notice,
                )
            }
            FormKind::WorldbookExport => (
                Command::Worldbook {
                    command: WorldbookCommand::Export {
                        worldbook: values[0].clone(),
                        output: values[1].clone().into(),
                    },
                },
                After::Notice,
            ),
            FormKind::Setup(vendor) => {
                let endpoint = values.get(1).map_or("", |value| value.trim());
                let (kind, key) = connection(vendor, values[0].trim(), endpoint)?;
                let old = Application::open(&self.path)?
                    .config
                    .providers
                    .remove(kind)
                    .map(|p| p.settings)
                    .unwrap_or_default();
                (
                    Command::Config {
                        command: ConfigCommand::Provider {
                            name: kind.into(),
                            kind: kind.into(),
                            model: if old.model.is_empty() {
                                DEFAULT_MODELS
                                    .iter()
                                    .find(|(vendor, _)| *vendor == kind)
                                    .map_or("default", |(_, model)| model)
                                    .into()
                            } else {
                                old.model
                            },
                            endpoint: (!endpoint.is_empty()).then(|| endpoint.into()),
                            api_key: key.map(str::to_owned),
                            context_limit: old.context_limit,
                            output_limit: old.output_limit,
                            parameters: serde_json::to_string(&old.parameters)?,
                            routing: old.routing.map(|r| serde_json::to_string(&r)).transpose()?,
                            merge_system_messages: old.merge_system_messages,
                        },
                    },
                    After::Setup(kind.into()),
                )
            }
            FormKind::Provider(_) => {
                if values[1] == "custom" && values[3].trim().is_empty() {
                    return Err(Error::new(
                        "invalid_input",
                        "The custom provider requires an endpoint.",
                    ));
                }
                if !["openrouter", "openai", "anthropic", "gemini", "custom"]
                    .contains(&values[1].as_str())
                {
                    return Err(Error::new("invalid_input", "The vendor is invalid."));
                }
                (
                    Command::Config {
                        command: ConfigCommand::Provider {
                            name: values[0].clone(),
                            kind: values[1].clone(),
                            model: values[2].clone(),
                            endpoint: (!values[3].is_empty()).then(|| values[3].clone()),
                            api_key: None,
                            context_limit: number(&values[4])?,
                            output_limit: number(&values[5])?,
                            parameters: values[6].clone(),
                            routing: (!values[7].is_empty()).then(|| values[7].clone()),
                            merge_system_messages: yes(&values[8])?,
                        },
                    },
                    After::Provider(values[0].clone()),
                )
            }
            FormKind::Import => {
                let value = values[0].trim();
                if value.starts_with("https://") || value.starts_with("http://") {
                    return self.fetch(value);
                }
                let path = local_path(value);
                if path.is_dir() {
                    return self.browse(path);
                }
                if !path.is_file() {
                    return Err(Error::new(
                        "invalid_input",
                        "The file does not exist. Select a PNG, JSON, or CHARX character file.",
                    ));
                }
                return self.open_card(path);
            }
            FormKind::Persona => (
                Command::Persona {
                    command: PersonaCommand::Create {
                        name: values[0].clone(),
                        description: Some(values[1].clone()),
                        description_file: None,
                        json: true,
                    },
                },
                After::Persona,
            ),
            FormKind::Export => (
                Command::Session {
                    command: SessionCommand::Export {
                        session: self.active()?,
                        output: values[0].clone().into(),
                        include_traces: yes(&values[1])?,
                    },
                },
                After::Notice,
            ),
            FormKind::Prompt => (
                Command::Prompt {
                    command: PromptCommand::Inspect {
                        session: self.active()?,
                        input: MessageInput {
                            message: Some(values[0].clone()),
                            message_file: None,
                        },
                        json: true,
                        request: true,
                    },
                },
                After::Request((!values[1].is_empty()).then(|| values[1].clone().into())),
            ),
            FormKind::Edit(id) => (
                Command::Session {
                    command: SessionCommand::Edit {
                        session: self.active()?,
                        at: id.clone(),
                        input: MessageInput {
                            message: Some(values[0].clone()),
                            message_file: None,
                        },
                    },
                },
                After::Say("The edit is saved. The original stays as an alternative."),
            ),
            FormKind::Rename => (
                Command::Session {
                    command: SessionCommand::Rename {
                        session: self.active()?,
                        title: values[0].clone(),
                    },
                },
                After::Say("The chat has the new name."),
            ),
            FormKind::Options => {
                let mut app = Application::open(&self.path)?;
                let mut prompt = app.config.prompt.clone().unwrap_or_default();
                prompt.history_limit = number(&values[0])?;
                prompt.memory_interval = Some(number(&values[1])?);
                prompt.context_limit = number(&values[2])?;
                prompt.output_limit = number(&values[3])?;
                prompt.lore_budget = number(&values[4])?;
                prompt.example_budget = number(&values[5])?;
                prompt.main_instruction = values[6].clone();
                prompt.post_history_instruction = values[7].clone();
                prompt.post_history_role = parse_role(&values[8])?;
                prompt.note = values[9].clone();
                prompt.note_depth = match values[10].trim() {
                    "" => None,
                    depth => Some(number(depth)?),
                };
                prompt.note_role = parse_role(&values[11])?;
                prompt.prefill = values[12].clone();
                if prompt.output_limit == 0 || prompt.context_limit <= prompt.output_limit {
                    return Err(Error::new(
                        "invalid_input",
                        "The context limit must be larger than the output limit.",
                    ));
                }
                app.config.prompt = Some(prompt);
                app.save_config()?;
                return self.apply("The prompt and memory settings are saved.");
            }
            FormKind::Agent => {
                let mut app = Application::open(&self.path)?;
                let mut policy = super::commands::agent_policy(&app.config)?;
                policy.max_steps = number(&values[0])?;
                if !(1..=64).contains(&policy.max_steps) {
                    return Err(Error::new(
                        "invalid_input",
                        "The tool steps must be from 1 to 64.",
                    ));
                }
                policy.confirm_tools = values[1]
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect();
                policy.retry_truncated = number(&values[2])?;
                policy.max_tokens = number(&values[3])? as u64;
                let table = serde_json::to_value(policy)?;
                app.config.plugins.entries.insert("agent".into(), table);
                app.save_config()?;
                return self.apply("The agent settings are saved.");
            }
            FormKind::Memory => (
                Command::Memory {
                    command: MemoryCommand::Edit {
                        session: self.active()?,
                        summary: Some(values[0].clone()),
                        facts: Some(values[1].clone()),
                    },
                },
                After::Say("The memory is saved."),
            ),
            FormKind::Bind => (
                Command::Plugin {
                    command: PluginCommand::Bind {
                        service: values[0].clone(),
                        plugin: values[1].clone(),
                    },
                },
                After::Notice,
            ),
            FormKind::Install => (
                Command::Plugin {
                    command: PluginCommand::Install {
                        path: values[0].clone().into(),
                        grant: values[1]
                            .lines()
                            .filter(|line| !line.is_empty())
                            .map(str::to_owned)
                            .collect(),
                        trust_processes: yes(&values[2])?,
                    },
                },
                After::Notice,
            ),
            FormKind::Run => (
                Command::Plugin {
                    command: PluginCommand::Run {
                        command: values[0].clone(),
                        input: CommandInput {
                            args: Some(values[1].clone()),
                            args_file: None,
                            url: None,
                        },
                    },
                },
                After::View("Plugin result"),
            ),
        };
        self.command(command, after);
        Ok(())
    }
    /// The method applies the saved settings to the open chat. The chat keeps its provider and model.
    fn apply(&mut self, notice: &'static str) -> Result<()> {
        let (Some(session), Some([model, provider])) = (&self.preferences.session, &self.model)
        else {
            self.notice = notice.into();
            return Ok(());
        };
        let command = SessionCommand::Configure {
            session: session.clone(),
            provider: Some(provider.clone()),
            model: Some(model.clone()),
            context_limit: None,
            output_limit: None,
            expected_version: None,
        };
        self.command(Command::Session { command }, After::Say(notice));
        Ok(())
    }
    /// The method selects the source plugin from the address.
    fn fetch(&mut self, url: &str) -> Result<()> {
        use rustytavern::source::{ChubSource, RisuRealmSource};
        let source = if ChubSource::resolve(url).is_ok() {
            "chub"
        } else if RisuRealmSource::resolve(url).is_ok() {
            "risurealm"
        } else {
            return Err(Error::new(
                "invalid_input",
                "RustyTavern downloads characters from chub.ai and realm.risuai.net only.",
            ));
        };
        let fetch = Command::Card {
            command: CardCommand::Fetch {
                url: url.into(),
                source: source.into(),
                json: true,
                preview: true,
            },
        };
        self.command(fetch, After::Preview(Box::new([url.into(), source.into()])));
        self.notice = "The character is downloading.".into();
        Ok(())
    }
    /// The method lists the subfolders and the character files of a folder.
    pub(super) fn browse(&mut self, folder: PathBuf) -> Result<()> {
        let mut entries: Vec<(bool, String)> = std::fs::read_dir(&folder)?
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let directory = entry.path().is_dir();
                let card = std::path::Path::new(&name)
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| {
                        ["png", "json", "charx"].contains(&&*value.to_ascii_lowercase())
                    });
                (!name.starts_with('.') && (directory || card)).then_some((!directory, name))
            })
            .collect();
        entries.sort_by_key(|(file, name)| (*file, name.to_lowercase()));
        let mut items = Vec::new();
        if let Some(parent) = folder.parent() {
            items.push(("↰ ..".into(), Choice::Browse(parent.into())));
        }
        for (file, name) in entries {
            let path = folder.join(&name);
            items.push(if file {
                (format!("  {}", text(&name)), Choice::Open(path))
            } else {
                (format!("▸ {}/", text(&name)), Choice::Browse(path))
            });
        }
        self.menu(
            &format!("Add a character · {}", text(&folder.to_string_lossy())),
            items,
        );
        self.folder = folder;
        self.notice = "Enter opens a folder or a card. Type to filter.".into();
        Ok(())
    }
    /// The method reads a character file and shows it. The store does not change.
    pub(super) fn open_card(&mut self, path: PathBuf) -> Result<()> {
        let bytes = crate::read_bytes(&path, rustytavern::card::MAX_INPUT_BYTES)?;
        let card = rustytavern::card::Card::import(&bytes)?;
        let import = Command::Card {
            command: CardCommand::Import { path, json: true },
        };
        self.preview(card, import);
        Ok(())
    }
    /// The method shows a character before the import. Enter runs `import`.
    pub(super) fn preview(&mut self, card: rustytavern::card::Card, import: Command) {
        let image = portrait(&card).map(|image| Box::new(self.picker.new_resize_protocol(image)));
        self.notice = "Review the character. Enter adds it.".into();
        self.overlay = Some(Overlay::Preview {
            card: Box::new(card),
            image,
            import: Box::new(import),
            tab: 0,
            scroll: 0,
        });
    }
}

fn role(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn parse_role(value: &str) -> Result<Role> {
    match value.trim() {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        _ => Err(Error::new(
            "invalid_input",
            "The role must be system, user, or assistant.",
        )),
    }
}

/// The function returns a field that wraps long lines.
fn wrapped(name: &'static str, value: &str) -> Field {
    let mut field = field(name, value);
    field
        .editor
        .set_wrap_mode(ratatui_textarea::WrapMode::WordOrGlyph);
    field
}

/// The function decodes the card picture. A PNG card is the picture. A CHARX card holds an icon asset.
fn portrait(card: &rustytavern::card::Card) -> Option<image::DynamicImage> {
    let bytes = if card.format == "png" {
        std::borrow::Cow::Borrowed(&card.original[..])
    } else {
        let assets = card.data()["assets"].as_array()?;
        let icons = || assets.iter().filter(|asset| asset["type"] == "icon");
        let icon = icons()
            .find(|asset| asset["name"] == "main")
            .or_else(|| icons().next())?;
        std::borrow::Cow::Owned(card.embedded_asset(icon["uri"].as_str()?).ok()?)
    };
    Some(image::load_from_memory(&bytes).ok()?.thumbnail(1024, 1024))
}

const DEFAULT_MODELS: &[(&str, &str)] = &[
    ("openrouter", "openrouter/auto"),
    ("openai", "gpt-5"),
    ("anthropic", "claude-sonnet-4-5"),
    ("gemini", "gemini-2.5-flash"),
];
/// Each vendor has an ID, a key prefix, a label, and a key page.
/// The OpenAI prefix comes last because it is the most general.
pub(super) const VENDORS: &[(&str, &str, &str, &str)] = &[
    (
        "openrouter",
        "sk-or-",
        "OpenRouter · one key for hundreds of models (recommended)",
        "openrouter.ai/keys",
    ),
    (
        "anthropic",
        "sk-ant-",
        "Anthropic · Claude",
        "console.anthropic.com/settings/keys",
    ),
    (
        "gemini",
        "AIza",
        "Google Gemini",
        "aistudio.google.com/apikey",
    ),
    (
        "openai",
        "sk-",
        "OpenAI · GPT",
        "platform.openai.com/api-keys",
    ),
];

/// The function returns the vendor and the pasted key.
/// A custom server can omit the key.
pub(super) fn connection<'a>(
    vendor: &str,
    key: &'a str,
    endpoint: &str,
) -> Result<(&'static str, Option<&'a str>)> {
    let invalid = |message: String| Error::new("invalid_input", message);
    let Some(&(kind, _, _, page)) = VENDORS.iter().find(|v| v.0 == vendor) else {
        if endpoint.is_empty() {
            return Err(invalid("Enter the server URL.".into()));
        }
        return Ok(("custom", (!key.is_empty()).then_some(key)));
    };
    if key.is_empty() {
        return Err(invalid(format!("Paste a key from {page}.")));
    }
    if let Some(other) = VENDORS.iter().find(|v| key.starts_with(v.1))
        && other.0 != kind
    {
        return Err(invalid(format!(
            "The key looks like a {} key. Go back and select {}.",
            other.0, other.0
        )));
    }
    Ok((kind, Some(key)))
}

/// The function removes quotes and shell escapes that terminals add when a file is dropped.
pub(super) fn local_path(value: &str) -> PathBuf {
    let quoted = value.len() >= 2
        && ['\'', '"']
            .iter()
            .any(|q| value.starts_with(*q) && value.ends_with(*q));
    let path = if quoted {
        value[1..value.len() - 1].to_owned()
    } else if cfg!(unix) {
        let mut chars = value.chars();
        let mut path = String::new();
        while let Some(c) = chars.next() {
            path.push(if c == '\\' {
                chars.next().unwrap_or(c)
            } else {
                c
            });
        }
        path
    } else {
        value.to_owned()
    };
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.into(),
    }
}
