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
    assert!(matches!(
        ui.overlay,
        Some(Overlay::Form {
            kind: FormKind::Provider,
            ..
        })
    ));
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
    key(&mut ui, "/setup");
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
    assert!(matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title=="Model profiles"));
    assert!(ui.pending.is_none());
    ui.preferences.provider = Some("local".into());
    ui.advance_new().unwrap();
    assert!(matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title=="Characters"));
    ui.preferences.character = Some("character".into());
    ui.advance_new().unwrap();
    assert!(matches!(ui.overlay,Some(Overlay::Menu{ref title,..}) if title=="Personas"));
    ui.preferences.persona = Some("persona".into());
    ui.advance_new().unwrap();
    assert!(matches!(
        ui.overlay,
        Some(Overlay::Form {
            kind: FormKind::New,
            ..
        })
    ));
    assert!(ui.pending.is_none());
}

#[test]
fn the_screen_removes_controls_and_handles_small_terminals() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.title = "Title\x1b[2J\x07".into();
    ui.notice = "Status\x1b[31m".into();
    ui.transcript = text("Reply\x1b]52;c;secret\x07");
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
    app.config.providers.insert("saved".into(), profile);
    app.save_config().unwrap();
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.preferences.provider = Some("saved".into());
    ui.form(FormKind::Provider);
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
    let mut ui = Ui::open(directory.path().into()).unwrap();
    ui.form(FormKind::Models("local".into()));
    ui.submit(
        &FormKind::Models("local".into()),
        &[
            "http://127.0.0.1:1/models".into(),
            "image".into(),
            "text".into(),
            "tools".into(),
        ],
    )
    .unwrap();
    assert!(matches!(
        ui.pending.take(),
        Some(Job::Command(
            Command::Model {
                command: ModelCommand::Discover {
                    input: Some(_),
                    parameter: Some(_),
                    ..
                }
            },
            After::Models(_)
        ))
    ));
    ui.complete(Ok(vec![json!({"provenance":{"protocol":"custom","endpoint":"http://127.0.0.1:1/models","fetched_at_unix_seconds":1,"evidence":"advertised_unverified"},"models":[{"id":"vision","name":null,"advertised":{"output_modalities":["text"]},"raw":{"future":42}}],"raw_pages":[]})]),After::Models("local".into()));
    assert!(ui.notice.contains("does not verify"));
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
