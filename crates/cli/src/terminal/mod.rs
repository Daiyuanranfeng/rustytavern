mod commands;
mod forms;
mod state;
mod view;

use crate::*;
use crossterm::{
    cursor::MoveTo,
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute, queue,
    terminal::{
        BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
        LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    DefaultTerminal, Terminal, TerminalOptions, Viewport,
    prelude::CrosstermBackend,
    widgets::{Paragraph, Widget},
};
use rustytavern::{
    app::PluginManager,
    config::{Application, default_home},
    runtime::RuntimeEvent,
    session::{Generation, GenerationMode},
};
use state::{After, Ui};
use std::io::{IsTerminal, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

enum Update {
    Command(Result<Vec<Value>>, After),
    Event(RuntimeEvent),
    Finished(Result<Generation>),
}

/// The flag records whether a dialog uses the alternate screen.
static ALTERNATE: AtomicBool = AtomicBool::new(false);

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
fn restore() {
    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    if ALTERNATE.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
    let _ = execute!(stdout(), crossterm::cursor::Show);
    let _ = disable_raw_mode();
}
/// The function opens an inline area at the cursor. The chat history stays in the native scrollback above it.
fn inline(height: u16) -> std::io::Result<DefaultTerminal> {
    Terminal::with_options(
        CrosstermBackend::new(stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
}
/// The function reads terminal input on a thread. Each short poll releases the input lock, so cursor queries can run.
fn input() -> mpsc::UnboundedReceiver<std::io::Result<Event>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while !sender.is_closed() {
            let event = match crossterm::event::poll(std::time::Duration::from_millis(50)) {
                Ok(false) => continue,
                Ok(true) => crossterm::event::read(),
                Err(error) => Err(error),
            };
            let failed = event.is_err();
            if sender.send(event).is_err() || failed {
                break;
            }
        }
    });
    receiver
}
/// The function draws one frame. A dialog uses the alternate screen with mouse capture.
/// The chat uses an inline area below the native scrollback. A synchronized update shows each frame at once.
fn present(
    ui: &mut Ui,
    terminal: &mut DefaultTerminal,
    screen: &mut Option<DefaultTerminal>,
) -> Result<()> {
    let mut rebuild = false;
    match (ui.overlay.is_some(), screen.is_some()) {
        (true, false) => {
            execute!(
                stdout(),
                EnterAlternateScreen,
                EnableMouseCapture,
                Clear(ClearType::All)
            )?;
            ALTERNATE.store(true, Ordering::SeqCst);
            *screen = Some(Terminal::new(CrosstermBackend::new(stdout()))?);
        }
        (false, true) => {
            *screen = None;
            execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
            ALTERNATE.store(false, Ordering::SeqCst);
            rebuild = true;
        }
        _ => {}
    }
    if let Some(full) = screen {
        full.draw(|frame| view::draw(frame, ui))?;
        return Ok(());
    }
    queue!(stdout(), BeginSynchronizedUpdate)?;
    terminal.autoresize()?;
    let (width, rows) = crossterm::terminal::size()?;
    let height = view::height(ui, width, rows);
    let area = terminal.get_frame().area();
    if rebuild || height != area.height {
        // The cursor query runs before the clear, so the area is never blank while the query waits.
        execute!(stdout(), MoveTo(0, area.y))?;
        *terminal = inline(height)?;
        queue!(
            stdout(),
            MoveTo(0, terminal.get_frame().area().y),
            Clear(ClearType::FromCursorDown)
        )?;
    }
    if !ui.history.is_empty() {
        let lines = view::history(&std::mem::take(&mut ui.history), width, ui.bubbles);
        let count = lines.len().min(usize::from(u16::MAX)) as u16;
        terminal.insert_before(count, |buffer| {
            Paragraph::new(lines).render(buffer.area, buffer)
        })?;
    }
    terminal.draw(|frame| view::draw(frame, ui))?;
    execute!(stdout(), EndSynchronizedUpdate)?;
    Ok(())
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
    enable_raw_mode()?;
    // The query reads standard input, so it runs before the input thread starts.
    if let Ok(picker) = ratatui_image::picker::Picker::from_query_stdio() {
        ui.picker = picker;
    }
    execute!(stdout(), EnableBracketedPaste)?;
    let mut terminal = inline(1)?;
    let mut screen: Option<DefaultTerminal> = None;
    let (sender, mut updates) = mpsc::channel(64);
    let mut events = input();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));
    let mut job: Option<tokio::task::JoinHandle<()>> = None;
    let mut cancellation = CancellationToken::new();
    let mut picture = false;
    let result: Result<()> = async {
        while !ui.exit || ui.busy {
            // Graphics protocols draw outside the cell buffer. A full redraw removes a hidden picture.
            let shown = matches!(
                ui.overlay,
                Some(state::Overlay::Preview { image: Some(_), tab: 0, .. })
            );
            if std::mem::replace(&mut picture, shown)
                && !shown
                && let Some(full) = screen.as_mut()
            {
                full.clear()?;
            }
            present(&mut ui, &mut terminal, &mut screen)?;
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
                event = events.recv() => {
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
                        state::Job::Generate { session, message, retry, mode } => { let result = generate(path, session, message, retry, mode, token, &sender).await; let _ = sender.send(Update::Finished(result)).await; }
                    }
                }));
            }
        }
        ui.save()
    }.await;
    if screen.take().is_some() {
        let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
        ALTERNATE.store(false, Ordering::SeqCst);
    }
    let top = terminal.get_frame().area().y;
    let _ = execute!(stdout(), MoveTo(0, top), Clear(ClearType::FromCursorDown));
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
    mode: GenerationMode,
    cancellation: CancellationToken,
    sender: &mpsc::Sender<Update>,
) -> Result<Generation> {
    let app = Application::open(path)?;
    let runtime = rustytavern::app::session_runtime(&app, &session).await?;
    let provider = runtime.provider_config(&session).await?;
    let mut handle = match mode {
        GenerationMode::Continue => runtime.continue_reply(&session, provider).await?,
        GenerationMode::Impersonate => runtime.impersonate(&session, provider).await?,
        GenerationMode::Reply if retry => runtime.regenerate(&session, provider).await?,
        GenerationMode::Reply => runtime.start(&session, &message, None, provider).await?,
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
