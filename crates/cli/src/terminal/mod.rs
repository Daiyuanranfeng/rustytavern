mod commands;
mod forms;
mod state;
mod view;

use crate::*;
use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
};
use futures_util::StreamExt;
use rustytavern::{
    app::{ExtensionManager, default_registry},
    config::{Application, default_data_dir},
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
fn restore() {
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
}

pub(super) async fn run(path: Option<PathBuf>) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::new(
            "terminal_required",
            "The interactive interface requires a terminal. Use --help for script commands.",
        ));
    }
    let path = path.map(Ok).unwrap_or_else(default_data_dir)?;
    let mut ui = Ui::open(path)?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(stdout(), EnableBracketedPaste)?;
    let (sender, mut updates) = mpsc::channel(64);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));
    let mut job: Option<tokio::task::JoinHandle<()>> = None;
    let mut cancellation = CancellationToken::new();
    let result: Result<()> = async {
        while !ui.exit || ui.busy {
            terminal.draw(|frame| view::draw(frame, &mut ui))?;
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
                    if !ui.busy { if let Some(task) = job.take() { let _ = task.await; } }
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
                            } else if !ui.busy { ui.key(key)?; }
                        }
                        Event::Paste(text) if !ui.busy => ui.paste(&text),
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
    let registry = default_registry(app.store.clone()).await?;
    let mut extensions = ExtensionManager::open(&app.data_dir, app.store, registry)?;
    extensions.restore().await?;
    let runtime = rustytavern::app::runtime(&extensions).await?;
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
