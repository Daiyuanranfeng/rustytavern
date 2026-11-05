use std::{
    io::{Read, Write},
    path::PathBuf,
};

use rustytavern::{
    Error, Result,
    app::{ExtensionManager, default_registry},
    card::{Card, MAX_INPUT_BYTES},
    config::{Application, default_data_dir},
    prompt::PromptOptions,
    provider::{ProviderConfig, ProviderKind},
    runtime::GenerationHandle,
    session::{GenerationStatus, Persona, Session},
    store::now_ms,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::*;

pub(super) async fn run(cli: Cli) -> Result<()> {
    execute(
        cli,
        &mut Output::Terminal,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
}

pub(super) async fn collect(
    data_dir: PathBuf,
    command: Command,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<Vec<Value>> {
    if matches!(
        command,
        Command::Chat { .. }
            | Command::Session {
                command: SessionCommand::Retry { .. } | SessionCommand::Regenerate { .. }
            }
    ) {
        return Err(Error::new(
            "invalid_command",
            "The terminal streams generations through the runtime.",
        ));
    }
    let mut output = Output::Values(Vec::new());
    execute(
        Cli {
            data_dir: Some(data_dir),
            command: Some(command),
        },
        &mut output,
        cancellation,
    )
    .await?;
    let Output::Values(values) = output else {
        unreachable!()
    };
    Ok(values)
}

enum Output {
    Terminal,
    Values(Vec<Value>),
}
impl Output {
    fn write(&mut self, value: &impl Serialize) -> Result<()> {
        match self {
            Self::Terminal => output(value),
            Self::Values(values) => {
                values.push(serde_json::to_value(value)?);
                Ok(())
            }
        }
    }
}

async fn execute(
    cli: Cli,
    output: &mut Output,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let command = cli
        .command
        .ok_or_else(|| Error::new("invalid_command", "The operation requires a command."))?;
    let path = match cli.data_dir {
        Some(path) => path,
        None => default_data_dir()?,
    };
    let mut app = Application::open(path)?;
    let registry = default_registry(app.store.clone()).await?;
    let mut extensions =
        ExtensionManager::open(&app.data_dir, app.store.clone(), registry.clone())?;
    let needs_runtime = matches!(
        &command,
        Command::Chat { .. }
            | Command::Prompt { .. }
            | Command::ExtensionAlias(_)
            | Command::Card {
                command: CardCommand::Fetch { .. } | CardCommand::Import { .. }
            }
            | Command::Extension {
                command: ExtensionCommand::Enable { .. } | ExtensionCommand::Run { .. }
            }
            | Command::Session {
                command: SessionCommand::New { .. }
                    | SessionCommand::Regenerate { .. }
                    | SessionCommand::Retry { .. }
                    | SessionCommand::Configure { .. }
            }
            | Command::Preset {
                command: PresetCommand::Inspect { resolved: true, .. }
            }
    );
    if needs_runtime {
        extensions.restore().await?;
    }
    let runtime = rustytavern::app::runtime(&extensions).await?;
    match command {
        Command::Model { command } => match command {
            ModelCommand::Discover {
                provider,
                endpoint,
                input,
                output: modality,
                parameter,
            } => {
                use rustytavern::provider::catalog::{CatalogClient, CatalogFilter};
                let config = extensions.provider(&app.config, &provider)?;
                let mut catalog = CatalogClient::default()
                    .fetch(
                        &config,
                        endpoint.as_deref(),
                        &rustytavern::provider::EnvironmentCredentials,
                        cancellation,
                    )
                    .await?;
                let filter = CatalogFilter {
                    input_modality: input,
                    output_modality: modality,
                    supported_parameter: parameter,
                };
                catalog.models.retain(|model| filter.matches(model));
                output.write(&catalog)?;
            }
            ModelCommand::Select { provider, model } => {
                use rustytavern::provider::ProviderAdapter;
                let mut config = extensions.provider(&app.config, &provider)?;
                let model = if config.kind == ProviderKind::Gemini {
                    model.strip_prefix("models/").unwrap_or(&model).to_owned()
                } else {
                    model
                };
                rustytavern::provider::HttpProvider.validate_config(&model, &config)?;
                config.model = model;
                app.config
                    .providers
                    .insert(provider.clone(), config.clone());
                app.save_config()?;
                output.write(&json!({"provider":provider,"configuration":config}))?;
            }
        },
        Command::Worldbook { command } => match command {
            WorldbookCommand::Import { path } => {
                let book = rustytavern::card::WorldBook::import(&read_bytes(
                    &path,
                    rustytavern::card::MAX_JSON_BYTES,
                )?)?;
                app.store.save_worldbook(&book)?;
                output.write(&json!({"id":book.id(),"document":book.document()}))?;
            }
            WorldbookCommand::Inspect { worldbook } => {
                let book = app.store.worldbook(&worldbook)?;
                output.write(&json!({"id":book.id(),"document":book.document()}))?;
            }
            WorldbookCommand::List => output.write(&app.store.list("worldbook")?)?,
            WorldbookCommand::Export {
                worldbook,
                output: path,
            } => {
                std::fs::write(path, app.store.worldbook(&worldbook)?.export_original())?;
                output.write(&json!({"exported":worldbook}))?;
            }
            WorldbookCommand::Bind {
                session,
                books,
                expected_version,
            } => {
                let target = app.store.session(&session)?;
                let mut configuration = target.configuration()?;
                configuration.worldbook_ids = books;
                output.write(&app.store.configure_session(
                    &session,
                    expected_version.unwrap_or(target.version),
                    configuration,
                )?)?;
            }
        },
        Command::Extension { command } => match command {
            ExtensionCommand::Install { path, builtin } => {
                let package = match (path, builtin) {
                    (Some(path), None) => extensions.install(&path)?,
                    (None, Some(name)) => extensions.install_builtin(&name)?,
                    _ => {
                        return Err(Error::new(
                            "invalid_extension_path",
                            "The install command requires a path or --builtin name.",
                        ));
                    }
                };
                output.write(&package)?;
            }
            ExtensionCommand::List { .. } => output.write(&extensions.list())?,
            ExtensionCommand::Inspect { extension, .. } => {
                output.write(&extensions.inspect(&extension)?)?
            }
            ExtensionCommand::Enable {
                extensions: ids,
                grant,
                trust_processes,
            } => {
                extensions
                    .enable(&ids, grant.into_iter().collect(), trust_processes)
                    .await?;
                output.write(&json!({"enabled":ids}))?;
            }
            ExtensionCommand::Disable { extension } => {
                extensions.disable(&extension).await?;
                output.write(&json!({"disabled":extension}))?;
            }
            ExtensionCommand::Uninstall { extension } => {
                extensions.uninstall(&extension)?;
                output.write(&json!({"uninstalled":extension}))?;
            }
            ExtensionCommand::Asset {
                extension,
                path,
                output: destination,
            } => {
                std::fs::write(&destination, extensions.asset(&extension, &path)?)?;
                output.write(&json!({"output":destination}))?;
            }
            ExtensionCommand::Run { command, input } => {
                let arguments = input.read()?;
                let cancellation = cancellation.clone();
                let result = cancel_operation(
                    extensions.run_command(&command, arguments, cancellation.clone()),
                    cancellation,
                )
                .await?;
                output.write(&result)?;
            }
        },
        Command::Plugin { command } => match command {
            PluginCommand::Inspect { plugin, .. } => {
                let mut records = registry
                    .lock()
                    .await
                    .inspect()
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                records.extend(extensions.declared_plugins()?);
                if let Some(id) = plugin {
                    output.write(
                        &records
                            .into_iter()
                            .find(|p| p["manifest"]["id"] == id)
                            .ok_or_else(|| Error::new("not_found", "The plugin does not exist."))?,
                    )?;
                } else {
                    output.write(&records)?;
                }
            }
            PluginCommand::Bind { service, plugin } => {
                extensions.bind(service.clone(), plugin.clone()).await?;
                output.write(&json!({"service":service,"plugin":plugin}))?;
            }
        },
        Command::Preset {
            command: PresetCommand::Inspect {
                preset, resolved, ..
            },
        } => {
            if preset != "tavern" {
                return Err(Error::new("invalid_preset", "The preset does not exist."));
            }
            if resolved {
                output.write(&extensions.composition().await?)?;
            } else {
                output.write(
                    &json!({"preset":preset,"standard_plugins":registry.lock().await.inspect(),"extensions":extensions.list()}),
                )?;
            }
        }
        Command::ExtensionAlias(arguments) => {
            let (name, rest) = arguments.split_first().ok_or_else(|| {
                Error::new(
                    "invalid_extension_command",
                    "The extension command name is missing.",
                )
            })?;
            let input = match rest {
                [] => json!({}),
                [url] => json!({"url":url}),
                [flag, url] if flag == "--url" => json!({"url":url}),
                [flag, value] if flag == "--args" => serde_json::from_str(value)?,
                _ => {
                    return Err(Error::new(
                        "invalid_command_arguments",
                        "The extension alias accepts a URL or --args JSON.",
                    ));
                }
            };
            let cancellation = cancellation.clone();
            output.write(
                &cancel_operation(
                    extensions.run_command(name, input, cancellation.clone()),
                    cancellation,
                )
                .await?,
            )?;
        }
        Command::Card { command } => match command {
            CardCommand::Import { path, .. } => {
                let card = runtime
                    .import_card(read_bytes(&path, MAX_INPUT_BYTES)?)
                    .await?;
                output.write(&inspect_card(&card))?;
            }
            CardCommand::Inspect { character, .. } => {
                output.write(&inspect_card(&app.store.card(&character)?))?
            }
            CardCommand::Asset {
                character,
                uri,
                output: path,
            } => {
                let bytes = app.store.card(&character)?.embedded_asset(&uri)?;
                std::fs::write(&path, bytes)?;
                output.write(&json!({"output":path}))?;
            }
            CardCommand::List { .. } => output.write(&app.store.list("card")?)?,
            CardCommand::Fetch { url, source, .. } => {
                let cancellation = cancellation.clone();
                let result = cancel_operation(
                    extensions.fetch_card(&url, &source, cancellation.clone()),
                    cancellation,
                )
                .await?;
                output.write(&inspect_card(&result))?;
            }
            CardCommand::Export {
                character,
                output: path,
                normalized,
                patch,
            } => {
                let card = app.store.card(&character)?;
                let bytes = if let Some(patch) = patch {
                    let update: Value =
                        serde_json::from_slice(&read_bytes(&patch, 8 * 1024 * 1024)?)?;
                    let mut document = card.document.clone();
                    merge_patch(&mut document, &update);
                    card.export_edited(&document)?
                } else if normalized {
                    serde_json::to_vec_pretty(&card.document)?
                } else {
                    card.export_original().to_vec()
                };
                std::fs::write(&path, bytes)?;
                output.write(&json!({"output":path}))?;
            }
        },
        Command::Persona { command } => match command {
            PersonaCommand::Export {
                persona,
                output: path,
            } => {
                let export = rustytavern::session::PersonaExport {
                    schema_version: 1,
                    persona: app.store.persona(&persona)?,
                };
                std::fs::write(&path, export.encode()?)?;
                output.write(&json!({"output":path,"schema_version":1}))?;
            }
            PersonaCommand::Create {
                name,
                description,
                description_file,
                ..
            } => {
                let description = match description_file {
                    Some(path) => read_text(&path)?,
                    None => description.unwrap_or_default(),
                };
                let persona = Persona::new(name, description)?;
                app.store.save_persona(&persona)?;
                output.write(&persona)?;
            }
            PersonaCommand::Show { persona, .. } => output.write(&app.store.persona(&persona)?)?,
            PersonaCommand::List { .. } => output.write(&app.store.list("persona")?)?,
        },
        Command::Session { command } => match command {
            SessionCommand::Configure {
                session,
                provider,
                model,
                context_limit,
                output_limit,
                expected_version,
            } => {
                let mut target = app.store.session(&session)?;
                target.provider = provider.unwrap_or(target.provider);
                let config = extensions.provider(&app.config, &target.provider)?;
                target.model = model.unwrap_or_else(|| {
                    if config.model.is_empty() {
                        target.model.clone()
                    } else {
                        config.model.clone()
                    }
                });
                target.options.context_limit = context_limit.unwrap_or(config.context_limit);
                target.options.output_limit = output_limit.unwrap_or(config.output_limit);
                target.version = expected_version.unwrap_or(target.version);
                output.write(&runtime.configure_session(&target, config).await?)?;
            }
            SessionCommand::AcceptDraft {
                session,
                generation,
                expected_version,
            } => {
                let version = expected_version.unwrap_or(app.store.session(&session)?.version);
                output.write(&app.store.accept_draft(&session, &generation, version)?)?;
            }
            SessionCommand::New {
                character,
                persona,
                provider,
                model,
                greeting,
                accept_reduced,
                ..
            } => {
                let card = app.store.card(&character)?;
                let config = extensions.provider(&app.config, &provider)?;
                let selected = if greeting == 0 {
                    card.text("first_mes").to_owned()
                } else {
                    card.data()["alternate_greetings"]
                        .as_array()
                        .and_then(|values| values.get(greeting - 1))
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            Error::new("invalid_greeting", "The greeting index does not exist.")
                        })?
                        .to_owned()
                };
                let model = model.unwrap_or(config.model.clone());
                if model.trim().is_empty() {
                    return Err(Error::new(
                        "configuration",
                        "The session requires a model ID.",
                    ));
                }
                let session = Session {
                    id: String::new(),
                    character_id: character,
                    persona_id: persona,
                    worldbook_ids: Vec::new(),
                    provider,
                    model,
                    head: None,
                    version: 0,
                    accept_reduced,
                    options: PromptOptions {
                        context_limit: config.context_limit,
                        output_limit: config.output_limit,
                        ..Default::default()
                    },
                    composition: json!({}),
                };
                output.write(
                    &runtime
                        .create_session(&session, Some(&selected), config)
                        .await?,
                )?;
            }
            SessionCommand::Show { session, .. } => {
                let generations = app.store.generations(&session)?.iter().map(|g| json!({"id":g.id,"user_node":g.user_node,"status":g.status,"draft":g.draft,"accepted_message":g.accepted_message,"usage":g.usage,"error":g.error})).collect::<Vec<_>>();
                output.write(
                    &json!({"session":app.store.session(&session)?,"history":app.store.history(&session)?,"messages":app.store.messages(&session)?,"generations":generations}),
                )?;
            }
            SessionCommand::List { .. } => output.write(&app.store.list("session")?)?,
            SessionCommand::Export {
                session,
                output: path,
                include_traces,
            } => {
                std::fs::write(
                    &path,
                    app.store
                        .export_session(&session, include_traces)?
                        .encode()?,
                )?;
                output.write(&json!({"output":path}))?;
            }
            SessionCommand::Regenerate { session, events }
            | SessionCommand::Retry { session, events } => {
                render(
                    runtime
                        .regenerate(&session, runtime.provider_config(&session).await?)
                        .await?,
                    events,
                )
                .await?;
            }
            SessionCommand::Branch { session, at } => {
                output.write(&app.store.select_head(&session, &at)?)?
            }
            SessionCommand::Edit { session, at, input } => {
                output.write(&app.store.edit(&session, &at, &input.read()?)?)?
            }
            SessionCommand::Rebind {
                session,
                character,
                persona,
                accept_reduced,
            } => output.write(&app.store.rebind(
                &session,
                character.as_deref(),
                persona.as_deref(),
                accept_reduced,
            )?)?,
            SessionCommand::Recover { session } => output.write(
                &json!({"interrupted_generations":app.store.recover(&session, now_ms())?}),
            )?,
        },
        Command::Prompt {
            command: PromptCommand::Replay { generation },
        } => output.write(&runtime.replay(&generation).await?)?,
        Command::Prompt {
            command:
                PromptCommand::Inspect {
                    session,
                    input,
                    request: full_request,
                    ..
                },
        } => {
            let request = runtime
                .inspect_request(
                    &session,
                    &input.read()?,
                    runtime.provider_config(&session).await?,
                )
                .await?;
            if full_request {
                output.write(&request)?;
            } else {
                output.write(&request.plan)?;
            }
        }
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                output.write(&extensions.configuration(&serde_json::to_value(&app.config)?)?)?
            }
            ConfigCommand::Capabilities => output.write(
                &[
                    ProviderKind::OpenRouter,
                    ProviderKind::OpenAi,
                    ProviderKind::Anthropic,
                    ProviderKind::Gemini,
                    ProviderKind::Custom,
                ]
                .map(ProviderKind::capabilities),
            )?,
            ConfigCommand::Provider {
                name,
                kind,
                model,
                endpoint,
                api_key_env,
                context_limit,
                output_limit,
                parameters,
                routing,
                merge_system_messages,
            } => {
                let (kind, default_env) = match kind.as_str() {
                    "openrouter" => (ProviderKind::OpenRouter, Some("OPENROUTER_API_KEY")),
                    "openai" => (ProviderKind::OpenAi, Some("OPENAI_API_KEY")),
                    "anthropic" => (ProviderKind::Anthropic, Some("ANTHROPIC_API_KEY")),
                    "gemini" => (ProviderKind::Gemini, Some("GEMINI_API_KEY")),
                    _ => (ProviderKind::Custom, None),
                };
                if name.trim().is_empty()
                    || model.trim().is_empty()
                    || context_limit <= output_limit
                    || output_limit == 0
                {
                    return Err(Error::new(
                        "configuration",
                        "The provider name, model, and limits are invalid.",
                    ));
                }
                let config = ProviderConfig {
                    kind,
                    model,
                    endpoint,
                    api_key_env: api_key_env.or_else(|| default_env.map(str::to_owned)),
                    context_limit,
                    output_limit,
                    parameters: serde_json::from_str(&parameters)?,
                    merge_system_messages,
                    routing: routing
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                    ..app.config.providers.get(&name).cloned().unwrap_or_default()
                };
                app.config.providers.insert(name.clone(), config);
                app.save_config()?;
                output.write(&json!({"provider":name}))?;
            }
        },
        Command::Chat {
            session,
            input,
            events,
            idempotency_key,
        } => {
            render(
                runtime
                    .start(
                        &session,
                        &input.read()?,
                        idempotency_key.as_deref(),
                        runtime.provider_config(&session).await?,
                    )
                    .await?,
                events,
            )
            .await?;
        }
    }
    Ok(())
}

async fn render(mut handle: GenerationHandle, events: bool) -> Result<()> {
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut cancelling = false;
    let mut rendered = String::new();
    let mut terminal_seen = false;
    loop {
        tokio::select! {
            _ = &mut interrupt, if !cancelling => { cancelling = true; handle.cancel(); }
            event = handle.events.recv() => match event {
                Some(event) => {
                    terminal_seen |= matches!(event.r#type.as_str(), "completed"|"failed"|"cancelled"|"interrupted");
                    if events { output(&event)?; }
                    else if event.r#type == "text_delta" && event.missed_events.is_none() {
                        let text = event.text.unwrap_or_default(); print!("{text}"); std::io::stdout().flush()?; rendered.push_str(&text);
                    }
                }
                None => break,
            }
        }
    }
    let generation = handle.wait().await?;
    if events && !terminal_seen {
        output(
            &json!({"type":match generation.status { GenerationStatus::Completed=>"completed",GenerationStatus::Cancelled=>"cancelled",GenerationStatus::Interrupted=>"interrupted",_=>"failed" },"generation_id":generation.id,"text":generation.draft,"error":generation.error,"missed_events":true}),
        )?;
    }
    if !events {
        if let Some(rest) = generation.draft.strip_prefix(&rendered) {
            print!("{rest}");
        }
        println!();
    }
    match generation.status {
        GenerationStatus::Completed => Ok(()),
        _ => Err(generation.error.unwrap_or_else(|| {
            Error::new("generation_failed", "The generation did not complete.")
        })),
    }
}

fn output(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
fn inspect_card(card: &Card) -> Value {
    json!({"id":card.id,"source_hash":card.source_hash,"format":card.format,"selected_envelope":card.selected_envelope,"conflicting_envelopes":card.conflicting_envelopes,"document":card.document,"diagnostics":card.diagnostics,"provenance":card.provenance,"decoder":card.decoder})
}
pub(super) fn read_bytes(path: &PathBuf, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if path.as_os_str() == "-" {
        std::io::stdin()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)?;
    } else {
        std::fs::File::open(path)?
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() > limit {
        return Err(Error::new(
            "invalid_input_size",
            "The input exceeds the byte limit.",
        ));
    }
    Ok(bytes)
}
pub(super) fn read_text(path: &PathBuf) -> Result<String> {
    String::from_utf8(read_bytes(path, 8 * 1024 * 1024)?)
        .map_err(|_| Error::new("invalid_utf8", "The input must contain UTF-8 text."))
}
fn merge_patch(target: &mut Value, patch: &Value) {
    if let Some(object) = patch.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in object {
            if value.is_null() {
                target.as_object_mut().map(|o| o.remove(key));
            } else {
                merge_patch(&mut target[key], value);
            }
        }
    } else {
        *target = patch.clone();
    }
}

async fn cancel_operation<T>(
    operation: impl std::future::Future<Output = Result<T>>,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<T> {
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        signal = tokio::signal::ctrl_c() => { signal?; cancellation.cancel(); operation.await },
        _ = cancellation.cancelled() => operation.await
    }
}
