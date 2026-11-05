mod commands;
mod forms;
mod state;
mod view;

use crate::*;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
};
use futures_util::StreamExt;
use rustytavern::{
    app::PluginManager,
    config::{Application, default_home},
    runtime::RuntimeEvent,
    session::Generation,
};
use state::{After, Ui};
use std::io::{IsTerminal, stdout};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

enum Update {
    Command(Result<Vec<Value>>, After),
    Event(RuntimeEvent),
    Finished(Result<Generation>),
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
/// The function sends text to the clipboard through OSC 52 and, on macOS, through pbcopy.
fn copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let _ = write!(stdout(), "\x1b]52;c;{encoded}\x07");
    let _ = stdout().flush();
    #[cfg(target_os = "macos")]
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}
fn restore() {
    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
}

pub(super) async fn run(path: Option<PathBuf>) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::new(
            "terminal_required",
            "The interactive interface requires a terminal. Use --help for script commands.",
        ));
    }
    let path = path.map(Ok).unwrap_or_else(default_home)?;
    let mut ui = Ui::open(path)?;
    if Application::open(&ui.path)?.config.providers.is_empty() {
        ui.slash("/provider")?;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let (sender, mut updates) = mpsc::channel(64);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));
    let mut job: Option<tokio::task::JoinHandle<()>> = None;
    let mut cancellation = CancellationToken::new();
    let result: Result<()> = async {
        while !ui.exit || ui.busy {
            terminal.draw(|frame| view::draw(frame, &mut ui))?;
            if let Some(text) = ui.copied.take() {
                copy(&text);
                ui.notice = format!("Copied {} characters.", text.chars().count());
            }
            tokio::select! {
                _ = tick.tick() => { ui.tick = ui.tick.wrapping_add(1); }
                signal = tokio::signal::ctrl_c() => { signal?; if ui.busy { cancellation.cancel(); } else { ui.exit=true; } }
                result = async { match job.as_mut() { Some(task)=>task.await, None=>std::future::pending().await } }, if ui.busy => {
                    job=None;
                    if let Err(error)=result { ui.busy=false;ui.notice=Error::new("terminal_task", "The terminal operation failed.").with_source(error).to_string();let _=ui.refresh(); }
                }
                update = updates.recv(), if ui.busy => {
                    match update {
                        Some(Update::Event(event)) => ui.event(event),
                        Some(Update::Command(result, after)) => { ui.complete(result, after); ui.busy = false; }
                        Some(Update::Finished(result)) => { ui.finish(result); ui.busy = false; }
                        None => return Err(Error::new("terminal_task", "The terminal operation ended without a result.")),
                    }
                    if !ui.busy && let Some(task) = job.take() { let _ = task.await; }
                }
                event = events.next() => {
                    let event = event.ok_or_else(|| Error::new("terminal_input", "The terminal input stream closed."))??;
                    match event {
                        Event::Key(key) if key.kind != KeyEventKind::Release => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d')) {
                                if ui.busy {
                                    cancellation.cancel(); ui.notice = "The runtime is saving cancellation.".into();
                                    if key.code == KeyCode::Char('d') { ui.exit = true; }
                                } else if key.code == KeyCode::Char('d') { ui.exit = true; }
                                else { ui.clear_input(); }
                            } else if !ui.busy || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o')) { ui.key(key)?; }
                        }
                        Event::Paste(text) if !ui.busy => ui.paste(&text),
                        Event::Mouse(mouse) => ui.mouse(mouse)?,
                        _ => {}
                    }
                }
            }
            if let Some(request) = ui.pending.take() {
                cancellation = CancellationToken::new();
                ui.busy = true;
                let sender = sender.clone();
                let path = ui.path.clone();
                let token = cancellation.clone();
                job = Some(tokio::spawn(async move {
                    match request {
                        state::Job::Command(command, after) => { let result = batch::collect(path, command, token).await; let _ = sender.send(Update::Command(result, after)).await; }
                        state::Job::Catalogs(providers) => {
                            let results = futures_util::future::join_all(providers.iter().map(|provider| {
                                let command = Command::Model { command: ModelCommand::Discover { provider: provider.clone(), endpoint: None, input: None, output: None, parameter: None } };
                                batch::collect(path.clone(), command, token.clone())
                            })).await;
                            let values = results.into_iter().map(|result| match result {
                                Ok(mut values) => values.pop().unwrap_or(Value::Null),
                                Err(error) => serde_json::json!({ "error": error.message }),
                            }).collect();
                            let _ = sender.send(Update::Command(Ok(values), state::After::Models(providers))).await;
                        }
                        state::Job::Generate { session, message, retry } => { let result = generate(path, session, message, retry, token, &sender).await; let _ = sender.send(Update::Finished(result)).await; }
                    }
                }));
            }
        }
        ui.save()
    }.await;
    cancellation.cancel();
    updates.close();
    if let Some(task) = job {
        let _ = task.await;
    }
    result
}

async fn generate(
    path: PathBuf,
    session: String,
    message: String,
    retry: bool,
    cancellation: CancellationToken,
    sender: &mpsc::Sender<Update>,
) -> Result<Generation> {
    let app = Application::open(path)?;
    let runtime = rustytavern::app::session_runtime(&app, &session).await?;
    let provider = runtime.provider_config(&session).await?;
    let mut handle = if retry {
        runtime.regenerate(&session, provider).await?
    } else {
        runtime.start(&session, &message, None, provider).await?
    };
    loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => { handle.cancel(); return handle.wait().await; }
            event = handle.events.recv() => match event {
                Some(event) => { if matches!(sender.try_send(Update::Event(event)),Err(mpsc::error::TrySendError::Closed(_))) { handle.cancel(); return handle.wait().await; } }
                None => return handle.wait().await,
            }
        }
    }
}
