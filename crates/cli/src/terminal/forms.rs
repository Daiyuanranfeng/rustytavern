use super::state::*;
use super::*;

impl Ui {
    pub(super) fn form(&mut self, kind: FormKind) {
        let (title, fields) = match &kind {
            FormKind::Setup(vendor) => {
                let mut fields = vec![Field {
                    secret: true,
                    ..field(
                        "API key, or the name of an environment variable that holds it",
                        "",
                    )
                }];
                if vendor == "custom" {
                    fields[0].name = "API key or environment variable: optional";
                    fields.push(field(
                        "Server URL, for example http://127.0.0.1:8080/v1/chat/completions",
                        "",
                    ));
                }
                ("Connect a model provider", fields)
            }
            FormKind::WorldbookImport => ("Import a worldbook", vec![field("File path", "")]),
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
            FormKind::Import => (
                "Add a character",
                vec![field(
                    "PNG, JSON, or CHARX file path, or a chub.ai or realm.risuai.net address",
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
        };
        if let FormKind::Setup(vendor) = &kind {
            self.notice = VENDORS.iter().find(|v| v.0 == vendor).map_or_else(
                || "Enter the server URL. A local server often needs no key.".into(),
                |v| {
                    format!(
                        "Paste a key from {}, or leave it empty to use {}.",
                        v.4, v.1
                    )
                },
            );
        }
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
                let (kind, reference, key) = connection(vendor, values[0].trim(), endpoint)?;
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
                            api_key_env: reference,
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
                            api_key_env: (!values[4].is_empty()).then(|| values[4].clone()),
                            api_key: None,
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
                let value = values[0].trim();
                if value.starts_with("https://") || value.starts_with("http://") {
                    return self.fetch(value);
                }
                let path = local_path(value);
                if !path.is_file() {
                    return Err(Error::new(
                        "invalid_input",
                        "The file does not exist. Select a PNG, JSON, or CHARX character file.",
                    ));
                }
                (
                    Command::Card {
                        command: CardCommand::Import { path, json: true },
                    },
                    After::Card,
                )
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
            },
        };
        self.command(fetch, After::Card);
        Ok(())
    }
}

const DEFAULT_MODELS: &[(&str, &str)] = &[
    ("openrouter", "openrouter/auto"),
    ("openai", "gpt-5"),
    ("anthropic", "claude-sonnet-4-5"),
    ("gemini", "gemini-2.5-flash"),
];
/// Each vendor has an ID, a default environment variable, a key prefix, a label, and a key page.
/// The OpenAI prefix comes last because it is the most general.
pub(super) const VENDORS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "openrouter",
        "OPENROUTER_API_KEY",
        "sk-or-",
        "OpenRouter · one key for hundreds of models (recommended)",
        "openrouter.ai/keys",
    ),
    (
        "anthropic",
        "ANTHROPIC_API_KEY",
        "sk-ant-",
        "Anthropic · Claude",
        "console.anthropic.com/settings/keys",
    ),
    (
        "gemini",
        "GEMINI_API_KEY",
        "AIza",
        "Google Gemini",
        "aistudio.google.com/apikey",
    ),
    (
        "openai",
        "OPENAI_API_KEY",
        "sk-",
        "OpenAI · GPT",
        "platform.openai.com/api-keys",
    ),
];

/// The function returns the vendor, an environment reference, and a pasted key.
/// An upper-case name is an environment variable. Other input is a key. An empty input uses the default variable.
pub(super) fn connection<'a>(
    vendor: &str,
    input: &'a str,
    endpoint: &str,
) -> Result<(&'static str, Option<String>, Option<&'a str>)> {
    let invalid = |message: String| Error::new("invalid_input", message);
    let variable = !input.is_empty()
        && input.starts_with(|c: char| c.is_ascii_uppercase())
        && input
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let exported = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let unset = |name: &str| {
        invalid(format!(
            "Paste a key, or export {name} and restart RustyTavern."
        ))
    };
    let Some(&(kind, default, ..)) = VENDORS.iter().find(|v| v.0 == vendor) else {
        if endpoint.is_empty() {
            return Err(invalid("Enter the server URL.".into()));
        }
        return match input {
            "" => Ok(("custom", None, None)),
            _ if variable => exported(input)
                .map(|_| ("custom", Some(input.into()), None))
                .ok_or_else(|| unset(input)),
            _ => Ok(("custom", None, Some(input))),
        };
    };
    let name = if input.is_empty() { default } else { input };
    let key = if input.is_empty() || variable {
        exported(name).ok_or_else(|| unset(name))?
    } else {
        input.to_owned()
    };
    if let Some(other) = VENDORS.iter().find(|v| key.starts_with(v.2))
        && other.0 != kind
    {
        return Err(invalid(format!(
            "The key looks like a {} key. Go back and select {}.",
            other.0, other.0
        )));
    }
    Ok(if input.is_empty() || variable {
        (kind, Some(name.into()), None)
    } else {
        (kind, None, Some(input))
    })
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
