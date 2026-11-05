use super::state::*;
use super::*;

impl Ui {
    pub(super) fn form(&mut self, kind: FormKind) {
        let (title, fields) = match &kind {
            FormKind::Models(_) => (
                "Discover model capabilities",
                vec![
                    field("Catalog endpoint: optional", ""),
                    field("Required input modality: optional", ""),
                    field("Required output modality: optional", ""),
                    field("Required parameter: optional", ""),
                ],
            ),
            FormKind::WorldbookImport => ("Import a worldbook", vec![field("File path", "")]),
            FormKind::WorldbookExport => (
                "Export a worldbook",
                vec![
                    field("Worldbook ID", ""),
                    field("Output file", "worldbook.json"),
                ],
            ),
            FormKind::Provider => {
                let name = self.preferences.provider.as_deref().unwrap_or("default");
                let selected = Application::open(&self.path)
                    .ok()
                    .and_then(|app| app.config.providers.get(name).cloned());
                let config = selected.clone().unwrap_or_default();
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
                        field(
                            "Credential environment variable: optional",
                            if selected.is_some() {
                                config.api_key_env.as_deref().unwrap_or_default()
                            } else {
                                ""
                            },
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
            FormKind::Import => ("Import a character", vec![field("File path", "")]),
            FormKind::Fetch => (
                "Fetch a character",
                vec![
                    field("Website URL", ""),
                    field("Enabled source: chub or risurealm", "chub"),
                ],
            ),
            FormKind::Persona => (
                "Create a persona",
                vec![field("Name", ""), field("Description", "")],
            ),
            FormKind::New => (
                "Start a conversation",
                vec![
                    field("Greeting index", "0"),
                    field("Accept preserved unsupported behavior: yes or no", "no"),
                ],
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
                "Install a product extension",
                vec![field("Package path or builtin: chub, risurealm", "")],
            ),
            FormKind::Enable(id) => (
                "Enable a product extension",
                vec![
                    field("Extension ID", id),
                    field("Explicit grants: one per line", ""),
                    field("Trust external processes: yes or no", "no"),
                ],
            ),
            FormKind::Run => (
                "Run an extension command",
                vec![field("Command name", ""), field("Arguments as JSON", "{}")],
            ),
        };
        self.overlay = Some(Overlay::Form {
            title: title.into(),
            kind,
            fields,
            selected: 0,
        });
    }
    pub(super) fn submit(&mut self, kind: &FormKind, values: &[String]) -> Result<()> {
        let yes = |value: &str| match value.trim() {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(Error::new("invalid_input", "The field requires yes or no.")),
        };
        let number = |value: &str| {
            value
                .parse::<usize>()
                .map_err(|_| Error::new("invalid_input", "The field requires an unsigned number."))
        };
        let (command, after) = match kind {
            FormKind::Models(provider) => {
                let optional = |index: usize| {
                    (!values[index].trim().is_empty()).then(|| values[index].trim().to_owned())
                };
                (
                    Command::Model {
                        command: ModelCommand::Discover {
                            provider: provider.clone(),
                            endpoint: optional(0),
                            input: optional(1),
                            output: optional(2),
                            parameter: optional(3),
                        },
                    },
                    After::Models(provider.clone()),
                )
            }
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
            FormKind::Provider => {
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
                            api_key_env: (!values[4].is_empty()).then(|| values[4].clone()),
                            context_limit: number(&values[5])?,
                            output_limit: number(&values[6])?,
                            parameters: values[7].clone(),
                            routing: (!values[8].is_empty()).then(|| values[8].clone()),
                            merge_system_messages: yes(&values[9])?,
                        },
                    },
                    After::Provider(values[0].clone()),
                )
            }
            FormKind::Import => {
                if values[0] == "-"
                    || !std::fs::metadata(&values[0]).is_ok_and(|metadata| metadata.is_file())
                {
                    return Err(Error::new(
                        "invalid_input",
                        "Select a regular character file.",
                    ));
                }
                (
                    Command::Card {
                        command: CardCommand::Import {
                            path: values[0].clone().into(),
                            json: true,
                        },
                    },
                    After::Card,
                )
            }
            FormKind::Fetch => (
                Command::Card {
                    command: CardCommand::Fetch {
                        url: values[0].clone(),
                        source: values[1].clone(),
                        json: true,
                    },
                },
                After::Card,
            ),
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
            FormKind::New => {
                self.new_requested = false;
                (
                    Command::Session {
                        command: SessionCommand::New {
                            character: self.preferences.character.clone().ok_or_else(|| {
                                Error::new("invalid_character", "Select a character.")
                            })?,
                            persona: self.preferences.persona.clone().ok_or_else(|| {
                                Error::new("invalid_persona", "Select a persona.")
                            })?,
                            provider: self.preferences.provider.clone().ok_or_else(|| {
                                Error::new("invalid_provider", "Select a provider.")
                            })?,
                            model: None,
                            greeting: number(&values[0])?,
                            accept_reduced: yes(&values[1])?,
                            json: true,
                        },
                    },
                    After::Session,
                )
            }
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
                Command::Extension {
                    command: ExtensionCommand::Install {
                        path: (!["chub", "risurealm"].contains(&values[0].as_str()))
                            .then(|| values[0].clone().into()),
                        builtin: (["chub", "risurealm"].contains(&values[0].as_str()))
                            .then(|| values[0].clone()),
                    },
                },
                After::Notice,
            ),
            FormKind::Enable(_) => (
                Command::Extension {
                    command: ExtensionCommand::Enable {
                        extensions: vec![values[0].clone()],
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
                Command::Extension {
                    command: ExtensionCommand::Run {
                        command: values[0].clone(),
                        input: CommandInput {
                            args: Some(values[1].clone()),
                            args_file: None,
                            url: None,
                        },
                    },
                },
                After::View("Extension result"),
            ),
        };
        self.command(command, after);
        Ok(())
    }
}
