use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn key(ui: &mut Ui, value: &str) {
    for character in value.chars() {
        ui.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
}

#[test]
fn slash_input_opens_controls_without_creating_a_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/setup");
    assert!(
        matches!(ui.overlay, Some(Overlay::Menu { ref title, ref items, .. }) if title == "Model providers · 0 connected" && items.len() == 5)
    );
    ui.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    key(&mut ui, "/not-a-command");
    assert!(ui.notice.starts_with("invalid_command"));
    assert!(ui.pending.is_none());
    assert!(
        Application::open(directory.path())
            .unwrap()
            .store
            .list("session")
            .unwrap()
            .is_empty()
    );
    key(&mut ui, "/quit");
    assert!(ui.exit);
}

#[test]
fn invalid_forms_keep_their_input_and_error_visible() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.form(FormKind::Provider("default".into()));
    if let Some(Overlay::Form { fields, .. }) = &mut ui.overlay {
        fields[1].editor = editor("unknown-vendor");
    }
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(ui.pending.is_none());
    assert!(matches!(ui.overlay, Some(Overlay::Form { .. })));
    let mut terminal = Terminal::new(TestBackend::new(100, 36)).unwrap();
    terminal
        .draw(|frame| super::super::view::draw(frame, &mut ui))
        .unwrap();
    let display = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(display.contains("The vendor is invalid."));
    assert!(display.contains("unknown-vendor"));
}

#[test]
fn new_conversations_require_explicit_identity_and_profile_selections() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/new");
    assert!(
        matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title.starts_with("Model providers"))
    );
    assert!(ui.pending.is_none());
    ui.preferences.provider = Some("local".into());
    ui.advance_new().unwrap();
    assert!(matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title=="Characters"));
    let card = std::fs::read("../core/tests/fixtures/full-v3.json").unwrap();
    let card = rustytavern::card::Card::import(&card).unwrap();
    Application::open(directory.path())
        .unwrap()
        .store
        .save_card(&card)
        .unwrap();
    ui.preferences.character = Some(card.id.clone());
    ui.advance_new().unwrap();
    assert!(matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title=="Personas"));
    ui.preferences.persona = Some("persona".into());
    ui.advance_new().unwrap();
    let Some(Overlay::Menu { title, items, .. }) = ui.overlay.take() else {
        panic!("The greeting menu is missing.");
    };
    assert!(title.starts_with("Choose how"));
    assert!(items.len() > 1);
    assert!(ui.pending.is_none());
    ui.choose(items.into_iter().nth(1).unwrap().1).unwrap();
    assert!(matches!(
        ui.pending,
        Some(Job::Command(
            Command::Session {
                command: SessionCommand::New { greeting: 1, .. }
            },
            After::Session
        ))
    ));
}

#[test]
fn the_screen_removes_controls_and_handles_small_terminals() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.title = "Title\x1b[2J\x07".into();
    ui.notice = "Status\x1b[31m".into();
    ui.outgoing = text("Reply\x1b]52;c;secret\x07");
    ui.note("Note\x1b[2J");
    let history = super::super::view::history(&ui.history, 40, ui.bubbles);
    assert!(
        !history
            .iter()
            .any(|line| line.to_string().contains(['\x1b', '\x07']))
    );
    for (width, height) in [(100, 30), (40, 12), (10, 3)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| super::super::view::draw(frame, &mut ui))
            .unwrap();
        let display = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!display.contains(['\x1b', '\x07']));
    }
}

#[test]
fn preferences_survive_restart_without_duplicate_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.preferences.provider = Some("selected".into());
    ui.save().unwrap();
    let reopened = Ui::open(directory.path().into()).unwrap();
    assert_eq!(reopened.preferences.provider.as_deref(), Some("selected"));
    assert!(reopened.preferences.session.is_none());
    assert_eq!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0
    );
}

#[test]
fn paste_limits_do_not_mutate_the_database_or_execute_commands() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.paste(&"x".repeat(128 * 1024 + 1));
    assert_eq!(ui.composer.lines(), [""]);
    ui.paste("/quit\nA pasted message");
    assert!(!ui.exit && ui.pending.is_none());
    assert!(
        Application::open(directory.path())
            .unwrap()
            .store
            .list("session")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn provider_forms_preserve_saved_settings_and_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = Application::open(directory.path()).unwrap();
    let profile = rustytavern::provider::ProviderConfig {
        model: "fixture".into(),
        timeout_seconds: 37,
        parameters: std::collections::BTreeMap::from([("temperature".into(), json!(0.4))]),
        merge_system_messages: true,
        ..Default::default()
    };
    app.config.set_provider("saved", profile, None);
    app.save_config().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.preferences.provider = Some("saved".into());
    ui.form(FormKind::Provider("saved".into()));
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let Some(Job::Command(command, after)) = ui.pending.take() else {
        panic!("The form does not submit a typed operation.")
    };
    let result =
        crate::batch::collect(directory.path().into(), command, CancellationToken::new()).await;
    ui.complete(result, after);
    let saved = Application::open(directory.path())
        .unwrap()
        .config
        .providers["saved"]
        .settings
        .clone();
    assert_eq!(saved.timeout_seconds, 37);
    assert_eq!(saved.parameters["temperature"], 0.4);
    assert!(saved.merge_system_messages);
}

#[test]
fn interactive_import_rejects_standard_input_before_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    assert_eq!(
        ui.submit(&FormKind::Import, &["-".into()])
            .unwrap_err()
            .code,
        "invalid_input"
    );
    assert!(ui.pending.is_none());
}

#[test]
fn request_exports_retain_values_beyond_the_terminal_display_limit() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    let value =
        json!({"prepared":{"body":{"messages":[{"role":"user","content":"x".repeat(140*1024)}]}}});
    let destination = directory.path().join("request.json");
    ui.complete(
        Ok(vec![value.clone()]),
        After::Request(Some(destination.clone())),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(destination).unwrap()).unwrap(),
        value
    );
    assert!(
        matches!(ui.overlay,Some(Overlay::Document{ref text,..}) if text.contains("The terminal view is limited."))
    );
}

#[test]
fn worldbook_controls_use_typed_commands_and_reject_terminal_stdin() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/worldbook");
    let Some(Overlay::Menu { items, .. }) = ui.overlay.take() else {
        panic!("The worldbook menu is missing.");
    };
    let (_, choice) = items.into_iter().next().unwrap();
    ui.choose(choice).unwrap();
    assert!(matches!(
        ui.overlay,
        Some(Overlay::Form {
            kind: FormKind::WorldbookImport,
            ..
        })
    ));
    assert_eq!(
        ui.submit(&FormKind::WorldbookImport, &["-".into()])
            .unwrap_err()
            .code,
        "invalid_input"
    );
    let path = directory.path().join("book.json");
    std::fs::write(&path, br#"{"entries":[]}"#).unwrap();
    ui.submit(&FormKind::WorldbookImport, &[path.to_string_lossy().into()])
        .unwrap();
    assert!(matches!(
        ui.pending,
        Some(Job::Command(
            Command::Worldbook {
                command: WorldbookCommand::Import { .. }
            },
            After::Notice
        ))
    ));
}

#[test]
fn model_discovery_uses_forms_and_does_not_grant_execution_capabilities() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = Application::open(directory.path()).unwrap();
    for name in ["local", "broken"] {
        app.config.set_provider(name, Default::default(), None);
    }
    app.save_config().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/model");
    assert!(
        matches!(ui.pending.take(), Some(Job::Catalogs(ref providers)) if providers == &["broken", "local"])
    );
    ui.complete(Ok(vec![json!({"error":"offline"}), json!({"provenance":{"protocol":"custom","endpoint":"http://127.0.0.1:1/models","fetched_at_unix_seconds":1,"evidence":"advertised_unverified"},"models":[{"id":"vision","name":null,"advertised":{"output_modalities":["text"]},"raw":{"future":42}}],"raw_pages":[]})]),After::Models(vec!["broken".into(), "local".into()]));
    assert!(ui.notice.contains("broken: offline"));
    let Some(Overlay::Menu { items, .. }) = ui.overlay.take() else {
        panic!("The model menu is missing.");
    };
    ui.choose(items.into_iter().next().unwrap().1).unwrap();
    assert!(matches!(
        ui.pending,
        Some(Job::Command(
            Command::Model {
                command: ModelCommand::Select { .. }
            },
            After::Provider(_)
        ))
    ));
    assert!(ui.preferences.session.is_none());
}

fn screen(ui: &mut Ui) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| super::super::view::draw(frame, ui))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..24)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn setup_checks_the_key_against_the_selected_provider() {
    use super::super::forms::connection;
    for (key, vendor) in [
        ("sk-or-v1-abc", "openrouter"),
        ("sk-ant-api03-abc", "anthropic"),
        ("AIzaSyabc", "gemini"),
        ("sk-proj-abc", "openai"),
    ] {
        assert_eq!(connection(vendor, key, "").unwrap(), (vendor, Some(key)));
    }
    assert!(
        connection("openai", "sk-or-v1-abc", "")
            .unwrap_err()
            .message
            .contains("looks like a openrouter key")
    );
    assert_eq!(
        connection("custom", "", "http://127.0.0.1:8080/v1/chat/completions").unwrap(),
        ("custom", None)
    );
    assert_eq!(
        connection("custom", "", "").unwrap_err().code,
        "invalid_input"
    );
    assert!(
        connection("openrouter", "", "")
            .unwrap_err()
            .message
            .contains("openrouter.ai/keys")
    );
}

#[tokio::test]
async fn setup_stores_a_pasted_key_in_the_configuration_file() {
    use rustytavern::provider::CredentialResolver;
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.submit(
        &FormKind::Setup("openrouter".into()),
        &["sk-or-v1-secret".into()],
    )
    .unwrap();
    let Some(Job::Command(command, after)) = ui.pending.take() else {
        panic!("The setup form does not submit a typed operation.")
    };
    let result =
        crate::batch::collect(directory.path().into(), command, CancellationToken::new()).await;
    ui.complete(result, after);
    assert!(
        matches!(ui.pending, Some(Job::Catalogs(ref providers)) if providers == &["openrouter"])
    );
    let config = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
    assert!(config.contains("api_key = \"sk-or-v1-secret\""));
    let app = Application::open(directory.path()).unwrap();
    let provider = app.config.provider("openrouter").unwrap();
    assert_eq!(provider.api_key_env.as_deref(), Some("config:openrouter"));
    assert_eq!(
        app.credentials().resolve("config:openrouter").unwrap(),
        "sk-or-v1-secret"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(directory.path().join("config.toml")).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn character_input_accepts_dropped_paths_and_detects_the_website() {
    use super::super::forms::local_path;
    assert_eq!(local_path("'/tmp/a b.png'"), PathBuf::from("/tmp/a b.png"));
    #[cfg(unix)]
    assert_eq!(local_path("/tmp/a\\ b.png"), PathBuf::from("/tmp/a b.png"));
    let directory = tempfile::tempdir().unwrap();
    let card = directory.path().join("my card.json");
    std::fs::copy("../core/tests/fixtures/full-v3.json", &card).unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.submit(&FormKind::Import, &[format!(" '{}' ", card.display())])
        .unwrap();
    assert!(matches!(ui.overlay, Some(Overlay::Preview { .. })));
    assert!(ui.pending.is_none());
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(
            Command::Card {
                command: CardCommand::Import { .. }
            },
            After::Card
        ))
    ));
    ui.submit(
        &FormKind::Import,
        &["https://chub.ai/characters/someone/someone-123".into()],
    )
    .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(
            Command::Card {
                command: CardCommand::Fetch { ref source, preview: true, .. }
            },
            After::Preview(..)
        )) if source == "chub"
    ));
    assert_eq!(
        ui.submit(&FormKind::Import, &["https://example.com/card".into()])
            .unwrap_err()
            .code,
        "invalid_input"
    );
}

#[test]
fn menus_filter_by_typing_and_activate_on_click() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/character");
    for character in "web".chars() {
        ui.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    let rows = screen(&mut ui);
    let row = rows
        .iter()
        .position(|row| row.contains("Add a character from a web address"))
        .unwrap();
    assert!(rows.iter().any(|row| row.contains("filter: web")));
    ui.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    })
    .unwrap();
    assert!(matches!(
        ui.overlay,
        Some(Overlay::Form {
            kind: FormKind::Import,
            ..
        })
    ));
}

#[test]
fn conversations_render_markdown_and_right_align_the_user() {
    use ratatui::style::Modifier;
    let lines = super::super::view::markdown("**bold** and *italic*\n\n- item");
    let span = |content: &str| {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == content)
            .unwrap()
            .style
    };
    assert!(span("bold").add_modifier.contains(Modifier::BOLD));
    assert!(span("italic").add_modifier.contains(Modifier::ITALIC));
    assert!(lines.iter().any(|line| line.to_string().contains("• item")));
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.busy = true;
    ui.streaming = true;
    ui.outgoing = "Hello there".into();
    let rows = screen(&mut ui);
    assert!(
        rows.iter()
            .any(|row| row.trim_end().ends_with("│ Hello there │") && row.starts_with("        "))
    );
    assert!(rows.iter().any(|row| row.starts_with("│ ·")));
    ui.streaming = false;
    ui.failure = Some("The provider failed.".into());
    assert!(
        screen(&mut ui)
            .iter()
            .any(|row| row.contains("⚠ The provider failed."))
    );
}

#[test]
fn model_lists_have_provider_tabs_and_put_new_models_first() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    let model = |id: &str, price: f64, created: u64| json!({"id":id,"name":null,"advertised":{"context_length":200000,"pricing":{"input":price,"output":price}},"raw":{"id":id,"created_at":created}});
    let catalog = |models: Vec<Value>| json!({"provenance":{"protocol":"openrouter","endpoint":"https://openrouter.ai/api/v1","fetched_at_unix_seconds":1,"evidence":"advertised_unverified"},"models":models});
    ui.complete(
        Ok(vec![
            catalog(vec![
                model("other/model", 0.0, 1700000000),
                model("anthropic/claude-sonnet-4.5", 3.0, 1759190400),
            ]),
            catalog(vec![model("claude-opus-9", 10.0, 1772236800)]),
        ]),
        After::Models(vec!["openrouter".into(), "anthropic".into()]),
    );
    let Some(Overlay::Menu { items, tabs, .. }) = &ui.overlay else {
        panic!("The model menu is missing.");
    };
    assert_eq!(
        tabs.iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        [
            "★ Recommended (1)",
            "All (3)",
            "openrouter (2)",
            "anthropic (1)"
        ]
    );
    assert_eq!(
        items[0].0,
        "anthropic  │   claude-opus-9 · 2026-02-28 · 200K context · $10.00 in, $10.00 out per 1M tokens"
    );
    assert_eq!(
        items[1].0,
        "openrouter │ ★ anthropic/claude-sonnet-4.5 · 2025-09-30 · 200K context · $3.00 in, $3.00 out per 1M tokens"
    );
    assert_eq!(
        items[2].0,
        "openrouter │   other/model · 2023-11-14 · 200K context · free"
    );
    let rows = screen(&mut ui);
    assert!(
        rows.iter()
            .any(|row| row.contains("★ Recommended (1)") && row.contains("anthropic (1)"))
    );
    ui.key(KeyCode::Right.into()).unwrap();
    ui.key(KeyCode::Enter.into()).unwrap();
    assert!(matches!(
        ui.pending,
        Some(Job::Command(Command::Model { command: ModelCommand::Select { ref model, .. } }, _)) if model == "claude-opus-9"
    ));
}

#[test]
fn the_header_shows_the_model_and_provider() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = Application::open(directory.path()).unwrap();
    app.config.set_provider(
        "openrouter",
        rustytavern::provider::ProviderConfig {
            model: "z-ai/glm-5".into(),
            ..Default::default()
        },
        None,
    );
    app.save_config().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    assert!(screen(&mut ui)[23].contains("No model"));
    ui.preferences.provider = Some("openrouter".into());
    ui.refresh().unwrap();
    assert!(screen(&mut ui)[23].contains("Model z-ai/glm-5 · Provider openrouter · for new chats"));
}

#[test]
fn a_partial_slash_command_runs_the_first_match() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    key(&mut ui, "/se");
    assert!(matches!(ui.overlay, Some(Overlay::Menu { ref title, .. }) if title == "Settings"));
}

#[test]
fn the_command_popup_selects_with_arrows_and_completes_with_tab() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    for character in "/c".chars() {
        ui.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    assert!(screen(&mut ui).iter().any(|row| row.contains("/chats")));
    ui.key(KeyCode::Down.into()).unwrap();
    ui.key(KeyCode::Tab.into()).unwrap();
    assert_eq!(ui.composer.lines(), ["/chats"]);
    ui.key(KeyCode::Enter.into()).unwrap();
    assert!(matches!(ui.overlay, Some(Overlay::Menu { ref title, .. }) if title == "Chats"));
}

#[tokio::test]
async fn the_scrollback_receives_each_stored_message_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = Application::open(directory.path()).unwrap();
    app.config.set_provider(
        "local",
        rustytavern::provider::ProviderConfig {
            model: "fixture".into(),
            ..Default::default()
        },
        None,
    );
    app.save_config().unwrap();
    let card = std::fs::read("../core/tests/fixtures/full-v3.json").unwrap();
    let card = rustytavern::card::Card::import(&card).unwrap();
    app.store.save_card(&card).unwrap();
    let persona = rustytavern::session::Persona::new("Alex".into(), String::new()).unwrap();
    app.store.save_persona(&persona).unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    assert!(ui.history[0].text.contains("Use /new"));
    ui.history.clear();
    ui.preferences.provider = Some("local".into());
    ui.preferences.character = Some(card.id.clone());
    ui.preferences.persona = Some(persona.id);
    ui.new_requested = true;
    ui.advance_new().unwrap();
    let Some(Overlay::Menu { items, .. }) = ui.overlay.take() else {
        panic!("The greeting menu is missing.");
    };
    ui.choose(items.into_iter().next().unwrap().1).unwrap();
    let Some(Job::Command(command, after)) = ui.pending.take() else {
        panic!("The greeting does not start a chat.")
    };
    let result =
        crate::batch::collect(directory.path().into(), command, CancellationToken::new()).await;
    ui.complete(result, after);
    assert!(ui.preferences.session.is_some(), "{}", ui.notice);
    let shown = ui.history.len();
    assert!(shown >= 2 && ui.history[0].label == "RustyTavern" && !ui.history[1].user);
    assert_eq!(ui.printed.len(), shown - 1);
    ui.history.clear();
    ui.refresh().unwrap();
    assert!(ui.history.is_empty());
    ui.printed.push("removed".into());
    ui.refresh().unwrap();
    assert!(
        ui.history[0]
            .text
            .contains("continues from an earlier message")
    );
    assert_eq!(ui.history.len(), 1);
}

#[test]
fn code_blocks_use_syntax_colors() {
    use ratatui::style::Color;
    let lines = super::super::view::markdown("```rust\nfn main() {}\n```\n\n```\nplain\n```");
    let colors: Vec<_> = lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter_map(|span| span.style.fg)
        .collect();
    assert!(colors.iter().any(|color| matches!(color, Color::Rgb(..))));
    assert!(
        lines
            .iter()
            .any(|line| line.to_string() == "  fn main() {}")
    );
    assert!(lines.iter().any(|line| line.to_string() == "  plain"));
}

#[test]
fn thinking_and_tool_steps_render_as_collapsible_bubbles_before_the_reply() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.streaming = true;
    ui.busy = true;
    ui.outgoing = "Find the weather.".into();
    let event = |kind: &str, value: &str| RuntimeEvent {
        r#type: kind.into(),
        generation_id: "g".into(),
        text: Some(value.into()),
        usage: None,
        error: None,
        missed_events: None,
        call_id: None,
    };
    ui.event(event("reasoning_delta", "The user wants "));
    ui.event(event("reasoning_delta", "the weather."));
    ui.event(event("tool_call", "web_fetch https://weather.example"));
    ui.event(event("tool_result", "Sunny, 21 °C"));
    ui.event(event("tool_call", "mcp__calendar__list"));
    ui.event(event("text_delta", "It is sunny."));
    assert_eq!(ui.steps.len(), 3);
    assert!(ui.steps[0].done && ui.steps[1].done && !ui.steps[2].done);
    let rows = screen(&mut ui);
    let order = [
        "▸ ✻ Thinking",
        "▸ ↗ web_fetch https://weather.example ✓",
        "▸ ⧉ mcp__calendar__list",
        "It is sunny.",
    ]
    .map(|needle| rows.iter().position(|row| row.contains(needle)).unwrap());
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!rows.iter().any(|row| row.contains("Sunny, 21")));
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(ui.steps.iter().all(|step| step.open));
    assert!(
        screen(&mut ui)
            .iter()
            .any(|row| row.contains("Sunny, 21 °C"))
    );
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(ui.steps.iter().all(|step| !step.open));
    ui.history.push(Message {
        user: false,
        label: "Character".into(),
        text: "It is sunny.".into(),
        steps: std::mem::take(&mut ui.steps),
    });
    let history: Vec<String> = super::super::view::history(&ui.history, 80, ui.bubbles)
        .iter()
        .map(ToString::to_string)
        .collect();
    let thinking = history
        .iter()
        .position(|row| row.contains("▸ ✻ Thinking"))
        .unwrap();
    let reply = history
        .iter()
        .position(|row| row.contains("It is sunny."))
        .unwrap();
    assert!(thinking < reply && !history.iter().any(|row| row.contains("Sunny, 21")));
}

/// The function returns a 4x2 PNG with a `chara` text chunk.
fn png_card(json: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let mut png = Vec::new();
    image::RgbImage::from_pixel(4, 2, image::Rgb([200, 40, 90]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut data = b"chara\0".to_vec();
    data.extend(
        base64::engine::general_purpose::STANDARD
            .encode(json)
            .bytes(),
    );
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    let body = [&b"tEXt"[..], &data].concat();
    chunk.extend(&body);
    chunk.extend(crc32fast::hash(&body).to_be_bytes());
    png.splice(33..33, chunk);
    png
}

#[test]
fn the_file_browser_lists_folders_and_cards_and_previews_before_import() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir_all(home.join("Cards")).unwrap();
    std::fs::create_dir_all(home.join(".hidden")).unwrap();
    std::fs::write(home.join("notes.txt"), "text").unwrap();
    let json = std::fs::read("../core/tests/fixtures/full-v3.json").unwrap();
    std::fs::write(home.join("Cards/Alice.PNG"), png_card(&json)).unwrap();
    std::fs::write(home.join("broken.json"), "{").unwrap();
    let mut ui = Ui::open(directory.path().join("data")).unwrap();
    ui.folder = home.clone();
    key(&mut ui, "/character");
    key(&mut ui, "from a file");
    let Some(Overlay::Menu {
        ref title,
        ref items,
        ..
    }) = ui.overlay
    else {
        panic!("The browser is not open.");
    };
    assert!(title.ends_with(&*home.to_string_lossy()));
    let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["↰ ..", "▸ Cards/", "  broken.json"]);
    key(&mut ui, "broken");
    assert!(matches!(ui.overlay, Some(Overlay::Menu { .. })));
    assert!(ui.notice.starts_with("invalid_card_json"));
    key(&mut ui, "cards");
    key(&mut ui, "alice");
    let Some(Overlay::Preview { ref image, .. }) = ui.overlay else {
        panic!("The preview is not open.");
    };
    assert!(image.is_some());
    assert!(ui.pending.is_none());
    let rows = screen(&mut ui).join("\n");
    assert!(rows.contains("Overview"));
    assert!(rows.contains("Lorebook (1)"));
    assert!(rows.contains("[ Add character ]"));
    ui.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
        .unwrap();
    assert!(screen(&mut ui).join("\n").contains("1 · First message"));
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(
            Command::Card {
                command: CardCommand::Import { ref path, .. }
            },
            After::Card
        )) if path.ends_with("Cards/Alice.PNG")
    ));
}

#[test]
fn downloaded_previews_fetch_the_card_again_on_enter() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    let json = std::fs::read("../core/tests/fixtures/full-v3.json").unwrap();
    let card = rustytavern::card::Card::import(&json).unwrap();
    ui.complete(
        Ok(vec![serde_json::to_value(&card).unwrap()]),
        After::Preview(Box::new([
            "https://chub.ai/characters/a/b".into(),
            "chub".into(),
        ])),
    );
    assert!(matches!(
        ui.overlay,
        Some(Overlay::Preview { image: None, .. })
    ));
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(
            Command::Card {
                command: CardCommand::Fetch { ref url, preview: false, .. }
            },
            After::Card
        )) if url == "https://chub.ai/characters/a/b"
    ));
    assert!(
        Application::open(directory.path())
            .unwrap()
            .store
            .list("card")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_empty_message_box_gives_keys_and_a_message_list_for_the_branch() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = Application::open(directory.path()).unwrap();
    app.config.set_provider(
        "local",
        rustytavern::provider::ProviderConfig {
            model: "fixture".into(),
            ..Default::default()
        },
        None,
    );
    app.save_config().unwrap();
    let card = std::fs::read("../core/tests/fixtures/full-v3.json").unwrap();
    let card = rustytavern::card::Card::import(&card).unwrap();
    app.store.save_card(&card).unwrap();
    let persona = rustytavern::session::Persona::new("Alex".into(), String::new()).unwrap();
    app.store.save_persona(&persona).unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.preferences.provider = Some("local".into());
    ui.preferences.character = Some(card.id.clone());
    ui.preferences.persona = Some(persona.id);
    ui.new_requested = true;
    ui.advance_new().unwrap();
    let Some(Overlay::Menu { items, .. }) = ui.overlay.take() else {
        panic!("The greeting menu is missing.");
    };
    ui.choose(items.into_iter().next().unwrap().1).unwrap();
    let Some(Job::Command(command, after)) = ui.pending.take() else {
        panic!("The greeting does not start a chat.")
    };
    let result =
        crate::batch::collect(directory.path().into(), command, CancellationToken::new()).await;
    ui.complete(result, after);
    let greeting = ui.head.clone().expect("The greeting is the head.");
    let bar = screen(&mut ui).join("\n");
    assert!(bar.contains("↑ edit") && bar.contains("/continue") && !bar.contains("Ctrl+R"));

    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    ui.key(esc).unwrap();
    assert!(ui.overlay.is_none() && ui.notice.contains("Esc again"));
    ui.key(esc).unwrap();
    assert!(
        matches!(ui.overlay, Some(Overlay::Menu { ref title, ref items, .. }) if title.starts_with("Messages in this branch") && items.len() == 1)
    );
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let Some(Overlay::Menu { items, .. }) = &ui.overlay else {
        panic!("The message actions are missing.");
    };
    let labels: Vec<_> = items.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Rewind to here",
            "Edit...",
            "Branch into a new chat",
            "Hide from the prompt",
            "Copy the text"
        ]
    );
    ui.key(esc).unwrap();

    ui.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .unwrap();
    let Some(Overlay::Form { kind, fields, .. }) = &ui.overlay else {
        panic!("The edit form is missing.");
    };
    assert!(matches!(kind, FormKind::Edit(id) if *id == greeting.id));
    assert_eq!(fields[0].editor.lines().join("\n"), greeting.text);
    ui.key(esc).unwrap();

    ui.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
        .unwrap();
    assert!(ui.pending.is_none(), "A greeting has no retry.");

    let session = ui.preferences.session.clone().unwrap();
    app.store.edit(&session, &greeting.id, "Edited.").unwrap();
    ui.refresh().unwrap();
    assert_eq!(ui.alternatives.len(), 2);
    assert!(screen(&mut ui).join("\n").contains("‹ 2/2 ›"));
    ui.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(Command::Session { command: SessionCommand::Branch { ref at, .. } }, _)) if *at == greeting.id
    ));

    ui.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();
    ui.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap();
    assert!(ui.pending.is_none());
    assert!(!screen(&mut ui).join("\n").contains("↑ edit"));
    ui.clear_input();

    // The test stores a user message as the head. A recovery releases the writer lease.
    app.store.select_head(&session, &greeting.id).unwrap();
    let user = app
        .store
        .prepare(rustytavern::store::Submit {
            session_id: &session,
            expected_version: app.store.session(&session).unwrap().version,
            text: "Hello.",
            idempotency_key: None,
            retry_user: None,
            mode: GenerationMode::Reply,
            snapshot: serde_json::json!({}),
            now: rustytavern::store::now_ms(),
            lease_ms: 1,
        })
        .unwrap()
        .generation
        .user_node;
    app.store.recover(&session, i64::MAX).unwrap();
    ui.refresh().unwrap();
    assert_eq!(ui.head.as_ref().map(|m| &m.id), Some(&user));
    assert!(screen(&mut ui).join("\n").contains("Ctrl+R reply"));
    ui.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Generate {
            retry: true,
            mode: GenerationMode::Reply,
            ..
        })
    ));
}

#[test]
fn tool_approvals_wait_in_order_and_a_draft_reset_replaces_the_reply() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.streaming = true;
    ui.busy = true;
    let event = |kind: &str, value: &str| RuntimeEvent {
        r#type: kind.into(),
        generation_id: "g".into(),
        text: Some(value.into()),
        usage: None,
        error: None,
        missed_events: None,
        call_id: Some("call_1".into()),
    };
    ui.event(event("text_delta", "Partial"));
    ui.event(event("draft_reset", ""));
    assert_eq!(ui.reply, "");
    // Parallel calls wait together. The terminal asks about each call in order.
    ui.event(event("tool_approval", "web_fetch {}"));
    ui.event(event("tool_approval", "read_skill {}"));
    assert_eq!(ui.approvals.len(), 2);
    assert!(ui.notice.contains("web_fetch"));
    ui.key(KeyCode::Char('y').into()).unwrap();
    assert!(ui.notice.contains("read_skill"));
    ui.key(KeyCode::Char('n').into()).unwrap();
    assert!(ui.approvals.is_empty());
    assert_eq!(ui.notice, "The tool call is declined.");
}
