//! Interactive chat-tui — Phase 1 (read-only ratatui dashboard).
//!
//! Goal: parity with codex/claude-code's terminal interactivity. Phase 1
//! gives users j/k navigation, Enter-to-expand task detail, and ? help —
//! the foundation we'll grow chat + slash commands + diff panels on top of
//! in subsequent phases. The non-interactive `mst tui --once` path stays
//! untouched so CI / scripts that consume the line dump keep working.
//!
//! Layout
//!   ┌─ status banner ────────────────────────────────────────────┐
//!   │ TASKS  (sidebar)         │ task detail (right pane)         │
//!   │ (j/k or ↑/↓ to nav)      │ status · duration · workspace    │
//!   │ Enter expands            │ log tail · context_bytes · skills│
//!   └─ help line ────────────────────────────────────────────────┘
//!
//! Auto-refreshes the on-disk RUN_STATE.json every `interval_ms` so the
//! live run's progress streams in. q / Ctrl-C / Esc all quit cleanly.

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chat::actions::{execute_action_with_session, Action, ActionStatus};
use crate::chat::sessions::{
    self as chat_sessions, Message as ChatMessage, Role as ChatRole, Session,
};
use crate::chat::stream::{send_streaming, StreamEvent};
use crate::paths;
use crate::scheduler::state::{RunState, RunStatus, TaskState, TaskStatus};
use tokio::sync::mpsc;

/// Entry point invoked by `cmd_tui` when `--interactive` is set.
pub async fn run(run: Option<String>, interval_ms: u64) -> Result<()> {
    // Resolve the run dir up front; if it disappears mid-loop we degrade to
    // a "no run" splash instead of crashing.
    let run_dir = resolve_run_dir(run.as_deref())?;

    // Refuse politely when not connected to a real terminal — crossterm
    // gives a cryptic "Failed to initialize input reader" otherwise, which
    // shows up in CI logs and asciinema headless mode and confuses users
    // into thinking the build is broken.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "--interactive needs a real terminal (stdin + stdout both tty). \
             Run it in your shell directly, not through pipes / asciinema / nohup. \
             Use `maestro tui` (no --interactive) for the line-printer dashboard \
             that works under pipes."
        );
    }

    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture).context("enter alt screen")?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend).context("create terminal")?;

    // Run the app, but always restore terminal state on the way out (even
    // on error or panic-via-?-propagation). The teardown block mirrors
    // ratatui's recommended cleanup sequence.
    let app_result = run_app(&mut terminal, run_dir, interval_ms).await;

    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();

    app_result
}

fn resolve_run_dir(run: Option<&str>) -> Result<PathBuf> {
    if let Some(id) = run {
        if id == "current" {
            return paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run"));
        }
        return paths::run_dir_for_id(id);
    }
    paths::current_run_dir()?.ok_or_else(|| anyhow::anyhow!("no current run"))
}

/// All UI state lives here so the render is a pure function of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Command,
    Chat,
}

#[derive(Clone, Copy)]
enum FlashKind {
    Info,
    Success,
    Error,
}

struct AppState {
    run_dir: PathBuf,
    state: Option<RunState>,
    load_error: Option<String>,
    selected: ListState,
    show_help: bool,
    last_refresh: Instant,
    interval: Duration,
    mode: Mode,
    /// Buffer for what the user is typing in Command mode (no leading `/`).
    cmd_input: String,
    /// Toast surfaced under the help line — outcome of the last action.
    flash: Option<(String, FlashKind, Instant)>,

    // --- Phase 3: chat panel ---
    /// The orchestrator chat session (lazy-loaded on first chat use).
    chat_session: Option<Session>,
    /// What the user is typing in Chat mode.
    chat_input: String,
    /// True while a turn is in-flight; blocks new sends.
    chat_streaming: bool,
    /// Receiver for StreamEvent frames coming out of send_streaming.
    chat_rx: Option<mpsc::Receiver<StreamEvent>>,
    /// In-flight assistant message — `(visible_text, thinking_text)`.
    /// Promoted to chat_session.messages on Done.
    chat_pending: Option<(String, String)>,
    /// When the current turn started (for the heartbeat under the bubble).
    chat_started_at: Option<Instant>,
    /// PageUp / PageDown scroll position into the chat transcript, in
    /// rendered-line units. 0 = pinned to the bottom (live tail). Grows
    /// as the user scrolls back; clamped on each render so a shrinking
    /// transcript can't strand them past the top. Replaces the old
    /// chat_visible_rows soft-cap — the viewport height + scroll offset
    /// is the right way to slice, not a fixed message count.
    chat_scroll: usize,

    // --- Phase 4: inline action cards ---
    /// Async notifier the action-decision tasks ping when they finish, so
    /// we can reload the session and re-render the new status.
    action_rx: Option<mpsc::Receiver<ActionUpdate>>,

    // --- Phase 5: diff preview side-panel ---
    /// `d` toggles a vertical split inside the body: detail | diff. Off by
    /// default so the default density matches Phase 1-4.
    show_diff: bool,
    /// Cache of `<task_id> -> diff text` so we don't shell out to git on
    /// every render. Invalidated when the user toggles diff off+on.
    diff_cache: std::collections::HashMap<String, String>,
}

struct ActionUpdate {
    action_id: String,
    final_status: ActionStatus,
    output: Option<String>,
}

impl AppState {
    fn new(run_dir: PathBuf, interval_ms: u64) -> Self {
        let mut selected = ListState::default();
        selected.select(Some(0));
        Self {
            run_dir,
            state: None,
            load_error: None,
            selected,
            show_help: false,
            last_refresh: Instant::now() - Duration::from_secs(60),
            interval: Duration::from_millis(interval_ms.max(100)),
            mode: Mode::Normal,
            cmd_input: String::new(),
            flash: None,
            chat_session: None,
            chat_input: String::new(),
            chat_streaming: false,
            chat_rx: None,
            chat_pending: None,
            chat_started_at: None,
            chat_scroll: 0,
            action_rx: None,
            show_diff: false,
            diff_cache: std::collections::HashMap::new(),
        }
    }

    /// Find the most-recent assistant message's first pending action,
    /// returning `(message_index, action_index)`. None when there is no
    /// pending action — Phase 4's `y` / `n` shortcuts target this.
    fn newest_pending_action(&self) -> Option<(usize, usize, String)> {
        let session = self.chat_session.as_ref()?;
        for (mi, m) in session.messages.iter().enumerate().rev() {
            for (ai, a) in m.actions.iter().enumerate() {
                if matches!(a.status, Some(ActionStatus::Pending) | None) {
                    return Some((mi, ai, a.id.clone()));
                }
            }
        }
        None
    }

    fn set_flash(&mut self, kind: FlashKind, msg: impl Into<String>) {
        self.flash = Some((msg.into(), kind, Instant::now()));
    }

    fn run_id(&self) -> Option<&str> {
        self.state.as_ref().map(|s| s.run_id.as_str())
    }

    fn ordered_tasks(&self) -> Vec<&TaskState> {
        let Some(s) = &self.state else {
            return vec![];
        };
        s.task_order
            .iter()
            .filter_map(|id| s.tasks.get(id))
            .collect()
    }

    fn selected_task(&self) -> Option<&TaskState> {
        let tasks = self.ordered_tasks();
        self.selected.selected().and_then(|i| tasks.get(i).copied())
    }

    fn move_selection(&mut self, delta: isize) {
        let n = self.ordered_tasks().len() as isize;
        if n == 0 {
            self.selected.select(None);
            return;
        }
        let cur = self.selected.selected().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(n);
        self.selected.select(Some(next as usize));
    }

    fn refresh_if_due(&mut self) {
        if self.last_refresh.elapsed() < self.interval {
            return;
        }
        match RunState::load(&self.run_dir) {
            Ok(s) => {
                self.state = Some(s);
                self.load_error = None;
            }
            Err(e) => {
                self.load_error = Some(format!("{e:#}"));
            }
        }
        self.last_refresh = Instant::now();
        // Clamp selection so a vanished task doesn't leave a dangling index.
        let n = self.ordered_tasks().len();
        if let Some(i) = self.selected.selected() {
            if n == 0 {
                self.selected.select(None);
            } else if i >= n {
                self.selected.select(Some(n - 1));
            }
        } else if n > 0 {
            self.selected.select(Some(0));
        }
    }
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    run_dir: PathBuf,
    interval_ms: u64,
) -> Result<()> {
    let mut app = AppState::new(run_dir, interval_ms);

    loop {
        app.refresh_if_due();
        drain_stream_events(&mut app);
        drain_action_updates(&mut app);
        terminal.draw(|f| draw(f, &mut app))?;

        // Block briefly so the screen doesn't hog CPU but we still feel
        // responsive to keypresses. 50ms is the sweet spot codex / lazygit
        // settle on too.
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c'));
                if ctrl_c {
                    return Ok(());
                }
                match app.mode {
                    Mode::Normal => {
                        if normal_mode_key(&mut app, key.code) == Outcome::Quit {
                            return Ok(());
                        }
                    }
                    Mode::Command => command_mode_key(&mut app, key.code),
                    Mode::Chat => chat_mode_key(&mut app, key.code),
                }
            }
        }
    }
}

/// Pull as many in-flight StreamEvents off the channel as are ready, so the
/// chat bubble re-renders smoothly without blocking the key-event poll.
fn drain_stream_events(app: &mut AppState) {
    let Some(rx) = app.chat_rx.as_mut() else {
        return;
    };
    loop {
        match rx.try_recv() {
            Ok(StreamEvent::Meta { .. }) => {
                if app.chat_pending.is_none() {
                    app.chat_pending = Some((String::new(), String::new()));
                }
            }
            Ok(StreamEvent::Delta { text }) => {
                let pending = app
                    .chat_pending
                    .get_or_insert_with(|| (String::new(), String::new()));
                pending.0.push_str(&text);
            }
            Ok(StreamEvent::Thinking { text }) => {
                let pending = app
                    .chat_pending
                    .get_or_insert_with(|| (String::new(), String::new()));
                pending.1.push_str(&text);
            }
            Ok(StreamEvent::Done { message }) => {
                // send_streaming has already persisted the session; reload
                // so we pick up the new message id + any side effects.
                if let Some(sess) = app.chat_session.as_ref() {
                    if let Ok(reloaded) = chat_sessions::load(&sess.id) {
                        app.chat_session = Some(reloaded);
                    } else {
                        // fallback: append to in-memory session
                        if let Some(s) = app.chat_session.as_mut() {
                            s.messages.push(message);
                        }
                    }
                }
                app.chat_pending = None;
                app.chat_streaming = false;
                app.chat_started_at = None;
                app.chat_rx = None;
                app.set_flash(FlashKind::Success, "turn complete");
                break;
            }
            Ok(StreamEvent::Error { message }) => {
                app.chat_pending = None;
                app.chat_streaming = false;
                app.chat_started_at = None;
                app.chat_rx = None;
                app.set_flash(FlashKind::Error, format!("chat error: {message}"));
                break;
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                app.chat_rx = None;
                if app.chat_streaming {
                    app.chat_streaming = false;
                    app.chat_started_at = None;
                    app.set_flash(FlashKind::Error, "chat stream closed unexpectedly");
                }
                break;
            }
        }
    }
}

/// Apply any completed action decisions to the session + flash a toast.
fn drain_action_updates(app: &mut AppState) {
    // Drain to a local Vec first so we don't hold a borrow into app while
    // calling app.set_flash later in the function.
    let mut updates: Vec<ActionUpdate> = Vec::new();
    let mut disconnected = false;
    if let Some(rx) = app.action_rx.as_mut() {
        loop {
            match rx.try_recv() {
                Ok(upd) => updates.push(upd),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }
    if disconnected {
        app.action_rx = None;
    }
    for upd in updates {
        // Reload session from disk — the action handler already persisted
        // the final status + output. Falls back to mutating the in-memory
        // copy if disk reload fails.
        if let Some(sess) = app.chat_session.as_ref() {
            if let Ok(reloaded) = chat_sessions::load(&sess.id) {
                app.chat_session = Some(reloaded);
            } else if let Some(s) = app.chat_session.as_mut() {
                for m in s.messages.iter_mut() {
                    for a in m.actions.iter_mut() {
                        if a.id == upd.action_id {
                            a.status = Some(upd.final_status);
                            a.output = upd.output.clone();
                        }
                    }
                }
            }
        }
        let (label, kind) = match upd.final_status {
            ActionStatus::Done => ("action done", FlashKind::Success),
            ActionStatus::Failed => ("action failed", FlashKind::Error),
            ActionStatus::Rejected => ("action rejected", FlashKind::Info),
            _ => ("action update", FlashKind::Info),
        };
        app.set_flash(kind, format!("{label} · {}", upd.action_id));
    }
}

/// Approve or reject an action (by id). Approve spawns the underlying
/// maestro subcommand in a tokio task; reject just stamps Rejected.
/// All state mutation goes through chat_sessions::save so the WebUI and
/// the TUI see the same status updates.
fn decide_action(app: &mut AppState, action_id: String, approve: bool) {
    let Some(session) = app.chat_session.clone() else {
        app.set_flash(FlashKind::Error, "no chat session");
        return;
    };

    // Locate + stamp the in-memory status. Save to disk so a concurrent
    // WebUI read sees the change immediately.
    let mut session = match chat_sessions::load(&session.id) {
        Ok(s) => s,
        Err(_) => session,
    };
    let mut hit: Option<Action> = None;
    'outer: for m in session.messages.iter_mut() {
        for a in m.actions.iter_mut() {
            if a.id == action_id {
                if approve {
                    a.status = Some(ActionStatus::Running);
                } else {
                    a.status = Some(ActionStatus::Rejected);
                }
                hit = Some(a.clone());
                break 'outer;
            }
        }
    }
    let Some(action) = hit else {
        app.set_flash(FlashKind::Error, format!("action {action_id} not found"));
        return;
    };
    let _ = chat_sessions::save(&session);
    app.chat_session = Some(session.clone());

    if !approve {
        app.set_flash(FlashKind::Info, format!("rejected {action_id}"));
        return;
    }

    // Approve path: spawn the subcommand, ping back on the action channel.
    let (tx, rx) = mpsc::channel::<ActionUpdate>(4);
    // Merge with any existing receiver — but in practice we only have one
    // action in flight at a time, so a fresh channel each time is fine.
    app.action_rx = Some(rx);
    let sid = session.id.clone();
    let aid = action.id.clone();
    app.set_flash(
        FlashKind::Info,
        format!("running action · {}", action.label),
    );
    tokio::spawn(async move {
        let res = execute_action_with_session(&action, Some(&sid)).await;
        // Persist final status to disk so the WebUI sees it too.
        if let Ok(mut sess) = chat_sessions::load(&sid) {
            for m in sess.messages.iter_mut() {
                for a in m.actions.iter_mut() {
                    if a.id == aid {
                        match &res {
                            Ok(out) => {
                                a.status = Some(ActionStatus::Done);
                                a.output = Some(out.clone());
                            }
                            Err(e) => {
                                a.status = Some(ActionStatus::Failed);
                                a.output = Some(format!("{e:#}"));
                            }
                        }
                    }
                }
            }
            let _ = chat_sessions::save(&sess);
        }
        let upd = ActionUpdate {
            action_id: aid,
            final_status: match &res {
                Ok(_) => ActionStatus::Done,
                Err(_) => ActionStatus::Failed,
            },
            output: match res {
                Ok(out) => Some(out),
                Err(e) => Some(format!("{e:#}")),
            },
        };
        let _ = tx.send(upd).await;
    });
}

#[derive(PartialEq, Eq)]
enum Outcome {
    Continue,
    Quit,
}

fn normal_mode_key(app: &mut AppState, code: KeyCode) -> Outcome {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return Outcome::Quit,
        KeyCode::Char('?') | KeyCode::Char('h') => app.show_help = !app.show_help,
        KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
        KeyCode::Char('g') | KeyCode::Home => app.selected.select(Some(0)),
        KeyCode::Char('G') | KeyCode::End => {
            let n = app.ordered_tasks().len();
            if n > 0 {
                app.selected.select(Some(n - 1));
            }
        }
        KeyCode::Char('r') => {
            // Force an immediate refresh.
            app.last_refresh = Instant::now() - Duration::from_secs(60);
        }
        // Vim/codex-style: ':' or '/' drops into Command mode. `/` keeps
        // the slash so user can keep typing the command name; `:` clears
        // it (vim feel — `:` is "ex mode" prefix, not part of the cmd).
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.cmd_input.clear();
        }
        KeyCode::Char('/') => {
            app.mode = Mode::Command;
            app.cmd_input.clear();
        }
        // Quick-action keys (codex/k9s convention: single letter on the
        // selected row). 'a' approves the highlighted task.
        KeyCode::Char('a') => {
            if let Some(t) = app.selected_task() {
                let id = t.id.clone();
                run_command(app, &format!("approve {id}"));
            }
        }
        // 'c' cancels the whole run after a confirmation prompt — for
        // now we drop into command mode pre-filled so user has to hit Enter.
        KeyCode::Char('c') => {
            app.mode = Mode::Command;
            app.cmd_input = "cancel".into();
        }
        // Tab / 'i' jumps into the chat input — codex/claude-code's
        // foot-pedal: dashboard nav is the default, talking to the agent
        // is one key away.
        KeyCode::Tab | KeyCode::Char('i') => {
            app.mode = Mode::Chat;
        }
        // PageUp / PageDown scrolls the chat panel without leaving
        // Normal mode — useful for catching up on streaming output
        // while keeping j/k available for task nav.
        KeyCode::PageUp => app.chat_scroll = app.chat_scroll.saturating_add(5),
        KeyCode::PageDown => app.chat_scroll = app.chat_scroll.saturating_sub(5),
        // 'd' toggles the diff side-panel for the highlighted task. Off
        // by default to keep the dashboard dense; on, the detail pane
        // halves and the right side shows the worktree diff vs HEAD.
        KeyCode::Char('d') => {
            app.show_diff = !app.show_diff;
            if !app.show_diff {
                // Drop the cache when toggling off so the next view shows
                // a fresh diff (cheap; only matters if user keeps a
                // long-lived TUI open through multiple task re-runs).
                app.diff_cache.clear();
            }
        }
        _ => {}
    }
    Outcome::Continue
}

fn chat_mode_key(app: &mut AppState, code: KeyCode) {
    // Inputs are blocked while a turn is streaming — pressing Enter again
    // could cause a double-send. The user can still Esc out to navigate.
    match code {
        KeyCode::Esc | KeyCode::Tab => {
            app.mode = Mode::Normal;
        }
        // Scroll the transcript without losing chat focus.
        KeyCode::PageUp => app.chat_scroll = app.chat_scroll.saturating_add(5),
        KeyCode::PageDown => app.chat_scroll = app.chat_scroll.saturating_sub(5),
        // Phase 4: y / n on the newest pending action (codex chat-tui
        // convention). Empty input → quick-decide; otherwise the char
        // joins the buffer so the user can type words starting with y/n.
        KeyCode::Char('y')
            if app.chat_input.is_empty()
                && !app.chat_streaming
                && app.newest_pending_action().is_some() =>
        {
            let (_, _, id) = app.newest_pending_action().unwrap();
            decide_action(app, id, true);
        }
        KeyCode::Char('n')
            if app.chat_input.is_empty()
                && !app.chat_streaming
                && app.newest_pending_action().is_some() =>
        {
            let (_, _, id) = app.newest_pending_action().unwrap();
            decide_action(app, id, false);
        }
        KeyCode::Enter if !app.chat_streaming => {
            let text = std::mem::take(&mut app.chat_input);
            let text = text.trim().to_string();
            if !text.is_empty() {
                start_chat_turn(app, text);
            }
        }
        KeyCode::Backspace if !app.chat_streaming => {
            app.chat_input.pop();
        }
        KeyCode::Char(c) if !app.chat_streaming => {
            app.chat_input.push(c);
        }
        _ => {}
    }
}

/// Kick off a chat turn: ensure a session exists, append the user message,
/// spawn the streaming task with a fresh channel. UI events arrive via
/// drain_stream_events on each tick — non-blocking.
fn start_chat_turn(app: &mut AppState, text: String) {
    let session = match ensure_chat_session(app) {
        Ok(s) => s,
        Err(e) => {
            app.set_flash(FlashKind::Error, format!("cannot open chat session: {e:#}"));
            return;
        }
    };
    let (tx, rx) = mpsc::channel::<StreamEvent>(64);
    app.chat_rx = Some(rx);
    app.chat_streaming = true;
    app.chat_started_at = Some(Instant::now());
    app.chat_pending = Some((String::new(), String::new()));

    // Optimistically append the user message to the in-memory session so
    // the UI shows the bubble immediately. send_streaming will persist
    // its own copy + the assistant reply.
    if let Some(s) = app.chat_session.as_mut() {
        s.messages
            .push(ChatMessage::new(ChatRole::User, text.clone()));
    }
    let session_for_send = session.clone();
    tokio::spawn(async move {
        if let Err(e) = send_streaming(session_for_send, text, tx.clone()).await {
            let _ = tx
                .send(StreamEvent::Error {
                    message: format!("{e:#}"),
                })
                .await;
        }
    });
}

fn ensure_chat_session(app: &mut AppState) -> anyhow::Result<Session> {
    if let Some(s) = app.chat_session.as_ref() {
        return Ok(s.clone());
    }
    let s = chat_sessions::ensure_current()?;
    app.chat_session = Some(s.clone());
    Ok(s)
}

fn command_mode_key(app: &mut AppState, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.cmd_input.clear();
        }
        KeyCode::Enter => {
            let cmd = std::mem::take(&mut app.cmd_input);
            app.mode = Mode::Normal;
            run_command(app, cmd.trim());
        }
        KeyCode::Backspace => {
            app.cmd_input.pop();
        }
        KeyCode::Char(c) => app.cmd_input.push(c),
        _ => {}
    }
}

/// Slash-command dispatcher. Input is the command name + space-separated
/// args (no leading `/`). Errors surface via the flash toast so the user
/// never has to leave the TUI to see what went wrong.
fn run_command(app: &mut AppState, raw: &str) {
    let raw = raw.trim().trim_start_matches('/');
    if raw.is_empty() {
        return;
    }
    let mut parts = raw.split_whitespace();
    let name = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    match name {
        "q" | "quit" | "exit" => {
            // The dispatch loop will see Mode::Quit-via-Esc; cheaper to
            // fake it by injecting an Esc on the next poll. Easier: just
            // bail by setting a sentinel flash and returning — the user
            // can hit q. Or: signal back to the run loop. For now,
            // simplest is to ask the user to hit q (consistent with vim's
            // behavior where :q errors if buffer is unsaved).
            app.set_flash(
                FlashKind::Info,
                "press q to quit (or Esc twice from command mode)",
            );
        }
        "help" | "h" | "?" => {
            app.show_help = !app.show_help;
        }
        "refresh" | "r" => {
            app.last_refresh = Instant::now() - Duration::from_secs(60);
            app.set_flash(FlashKind::Info, "refreshed");
        }
        "approve" | "a" => {
            let target = args
                .first()
                .copied()
                .map(String::from)
                .or_else(|| app.selected_task().map(|t| t.id.clone()));
            match target {
                None => app.set_flash(FlashKind::Error, "/approve needs a task id (or select one)"),
                Some(id) => match approve_task(&id) {
                    Ok(()) => {
                        app.set_flash(FlashKind::Success, format!("approved {id}"));
                        app.last_refresh = Instant::now() - Duration::from_secs(60);
                    }
                    Err(e) => app.set_flash(FlashKind::Error, format!("/approve failed: {e:#}")),
                },
            }
        }
        "cancel" => {
            let run_id = app.run_id().unwrap_or("current").to_string();
            match cancel_run(&run_id, &app.run_dir) {
                Ok(()) => {
                    app.set_flash(
                        FlashKind::Success,
                        format!("cancel marker written for {run_id} (force-cancel applied if owner was dead)"),
                    );
                    app.last_refresh = Instant::now() - Duration::from_secs(60);
                }
                Err(e) => app.set_flash(FlashKind::Error, format!("/cancel failed: {e:#}")),
            }
        }
        "logs" => {
            // Phase 2 simple form: just print "open this file" hint —
            // proper inline log tailing lands in Phase 3 with the chat
            // panel infrastructure. The runs/<id>/<task>.log path is
            // shown so the user can `tail -f` it in another pane.
            let target = args
                .first()
                .copied()
                .map(String::from)
                .or_else(|| app.selected_task().map(|t| t.id.clone()));
            match target {
                None => app.set_flash(FlashKind::Error, "/logs needs a task id (or select one)"),
                Some(id) => {
                    let path = app.run_dir.join(format!("{id}.log"));
                    app.set_flash(FlashKind::Info, format!("log: {}", path.display()));
                }
            }
        }
        "rerun" | "replay" => {
            // Both require launching the maestro CLI itself — Phase 2 just
            // tells the user the exact command (codex/claude code style:
            // commands that change *the world* surface the underlying
            // CLI invocation so they're auditable + scriptable).
            let run_id = app.run_id().unwrap_or("current");
            let cmd = if name == "rerun" {
                format!("maestro rerun {run_id}")
            } else {
                format!("maestro runs replay {run_id}")
            };
            app.set_flash(FlashKind::Info, format!("run in another terminal: {cmd}"));
        }
        unknown => {
            app.set_flash(
                FlashKind::Error,
                format!("unknown command `/{unknown}` — try /approve /cancel /logs /rerun /replay /help /quit"),
            );
        }
    }
}

/// Write the approval marker exactly like `mst approve <id>` does.
fn approve_task(task_id: &str) -> anyhow::Result<()> {
    let dir = paths::approvals_dir()?;
    paths::ensure_dir(&dir)?;
    let f = paths::control_marker_path(&dir, "task id", task_id)?;
    std::fs::write(&f, b"approved").context("write approval marker")
}

/// Cancel exactly like `mst cancel-run <id>` does — marker + abandoned-run safety net.
fn cancel_run(run_id: &str, run_dir: &std::path::Path) -> anyhow::Result<()> {
    let dir = paths::cancels_dir()?;
    paths::ensure_dir(&dir)?;
    let marker = paths::control_marker_path(&dir, "run id", run_id)?;
    std::fs::write(&marker, b"cancel").context("write cancel marker")?;
    // Safety net for dead owners — same logic the CLI + web handler use.
    let _ = crate::scheduler::force_cancel_if_abandoned(run_dir)?;
    Ok(())
}

fn draw(f: &mut Frame, app: &mut AppState) {
    // When chat mode is active the chat panel grows to take roughly half
    // the body, mirroring codex/claude-code's "expand the input when
    // focused" feel. In Normal mode it stays a slim recap (last few
    // messages) so the dashboard reads as the primary content.
    let chat_height = if app.mode == Mode::Chat {
        (f.area().height / 2).max(10)
    } else if app.chat_session.is_some() || app.chat_streaming || app.chat_pending.is_some() {
        // We've chatted before — keep a slim transcript visible.
        8
    } else {
        // No chat yet — collapse to a one-line "Tab to chat" hint.
        3
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),           // banner
            Constraint::Min(8),              // dashboard body
            Constraint::Length(chat_height), // chat panel
            Constraint::Length(1),           // help line
        ])
        .split(f.area());

    draw_banner(f, chunks[0], app);
    draw_body(f, chunks[1], app);
    draw_chat_panel(f, chunks[2], app);
    draw_help_line(f, chunks[3], app);

    if app.show_help {
        draw_help_overlay(f, f.area());
    }
}

fn draw_chat_panel(f: &mut Frame, area: Rect, app: &mut AppState) {
    let title = if app.chat_streaming {
        let elapsed = app
            .chat_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        format!(" chat · streaming {elapsed}s ")
    } else if app.mode == Mode::Chat {
        " chat · type to send ".to_string()
    } else {
        " chat ".to_string()
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Empty-state hint — no session yet AND nothing in flight.
    if app.chat_session.is_none() && !app.chat_streaming {
        let hint = if app.mode == Mode::Chat {
            "(press Enter to send · Esc/Tab to leave chat)"
        } else {
            "(Tab or i to chat with the orchestrator)"
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    // Layout the chat region: transcript (rest) + input box (1 line).
    let lay = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    // Render transcript: ALL messages from the session — the scroll
    // window (lay[0].height + chat_scroll) decides what's visible. This
    // is the correct shape for PageUp/PageDown nav; clipping to the
    // last N would silently lose backbuffer the user just paged into.
    let mut lines: Vec<Line> = Vec::new();
    if let Some(sess) = &app.chat_session {
        for m in &sess.messages {
            lines.extend(chat_message_lines(m, area.width as usize));
        }
    }
    if let Some((visible, thinking)) = &app.chat_pending {
        // Thinking trace shown above the answer in DarkGray (the chat
        // bubble equivalent of the WebUI's collapsible details).
        if !thinking.is_empty() {
            for tl in thinking.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    💭 {tl}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        if !visible.is_empty() {
            for vl in visible.lines() {
                lines.push(Line::from(vec![
                    Span::styled("🤖  ", Style::default().fg(Color::Cyan)),
                    Span::raw(vl.to_string()),
                ]));
            }
        } else if thinking.is_empty() && app.chat_streaming {
            lines.push(Line::from(Span::styled(
                "🤖  thinking…",
                Style::default().fg(Color::Cyan),
            )));
        }
    }
    // Slice the visible window. `chat_scroll = 0` pins to the bottom;
    // larger values reveal earlier lines. Clamp here so the user can't
    // strand themselves past line zero on a shrinking transcript.
    let viewport = lay[0].height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(viewport);
    if app.chat_scroll > max_scroll {
        app.chat_scroll = max_scroll;
    }
    let end = total.saturating_sub(app.chat_scroll);
    let start = end.saturating_sub(viewport);
    let visible: Vec<Line> = lines[start..end].to_vec();
    let para = Paragraph::new(visible).wrap(Wrap { trim: false });
    f.render_widget(para, lay[0]);

    // Input box
    let cursor = if app.mode == Mode::Chat { "█" } else { "" };
    let prompt_color = if app.mode == Mode::Chat {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let input_line = Line::from(vec![
        Span::styled(
            "> ",
            Style::default()
                .fg(prompt_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.chat_input.clone()),
        Span::styled(cursor, Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(input_line), lay[1]);
}

/// Render one persisted chat message as a series of lines, with role
/// prefix + role-coded color. Multi-line content is wrapped onto extra
/// lines with leading indent so the role icon doesn't repeat per row.
/// Phase 4 additions: hides any `maestro-action` fenced YAML blocks
/// from the body (they render as inline cards below instead), and
/// appends one card per action attached to the message.
fn chat_message_lines(m: &ChatMessage, _max_width: usize) -> Vec<Line<'static>> {
    let (icon, color) = match m.role {
        ChatRole::User => ("🧑  ", Color::Yellow),
        ChatRole::Assistant => ("🤖  ", Color::Cyan),
        ChatRole::System => ("⚙️  ", Color::DarkGray),
    };
    let mut out = Vec::new();
    if let Some(thinking) = &m.thinking {
        for tl in thinking.lines() {
            out.push(Line::from(Span::styled(
                format!("    💭 {tl}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    // Strip the fenced action blocks from the visible text — they're
    // rendered as cards below; leaving them inline would be redundant
    // and screen-real-estate hostile in a TUI.
    let body = strip_action_fences(&m.content);
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() && m.actions.is_empty() {
        out.push(Line::from(Span::styled(icon, Style::default().fg(color))));
    } else {
        let mut in_code_block = false;
        for (i, l) in lines.iter().enumerate() {
            // Track fenced code blocks so their content renders monospace-
            // gray and bullets/headings inside aren't accidentally styled.
            if l.trim_start().starts_with("```") {
                in_code_block = !in_code_block;
                let prefix = if i == 0 {
                    Span::styled(
                        icon,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::raw("    ")
                };
                out.push(Line::from(vec![
                    prefix,
                    Span::styled(l.to_string(), Style::default().fg(Color::DarkGray)),
                ]));
                continue;
            }
            let prefix = if i == 0 {
                Span::styled(
                    icon,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("    ")
            };
            let mut spans = vec![prefix];
            spans.extend(render_markdown_inline(l, in_code_block));
            out.push(Line::from(spans));
        }
    }
    for a in &m.actions {
        out.extend(action_card_lines(a));
    }
    out
}

/// Tiny markdown formatter for a single chat line. Handles:
///   * `# ` / `## ` / `### ` headings   → bold accent color
///   * `- ` / `* ` list bullets         → `•` accent + indent
///   * `> ` quotes                       → faded gray
///   * inline `` `code` ``               → green monospace chip
///   * `**bold**` / `*italic*`           → bold / italic modifiers
///
/// Plain text fast-path: if a line has no markdown markers it returns a
/// single Span — no allocation explosion on long answers. When inside a
/// fenced code block (`in_code` true), the line is rendered as-is in
/// DarkGray so code never accidentally tries to interpret `# ` as a
/// heading.
fn render_markdown_inline(line: &str, in_code: bool) -> Vec<Span<'static>> {
    if in_code {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(Color::DarkGray),
        )];
    }
    // Heading detection (only at start of line)
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("### ") {
        return vec![Span::styled(
            rest.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )];
    }
    if let Some(rest) = trimmed.strip_prefix("## ") {
        return vec![Span::styled(
            rest.to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )];
    }
    if let Some(rest) = trimmed.strip_prefix("# ") {
        return vec![Span::styled(
            rest.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )];
    }
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        let mut spans = vec![Span::styled(
            "  • ".to_string(),
            Style::default().fg(Color::Cyan),
        )];
        spans.extend(render_inline_emphasis(rest));
        return spans;
    }
    if let Some(rest) = trimmed.strip_prefix("> ") {
        return vec![Span::styled(
            format!("  ┊ {rest}"),
            Style::default().fg(Color::DarkGray),
        )];
    }
    render_inline_emphasis(line)
}

/// Inline emphasis pass: scans for `**bold**`, `*italic*`, `` `code` ``
/// and emits styled spans. Falls back to a single raw span when nothing
/// matches (the common case for plain text).
fn render_inline_emphasis(line: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        // Pick the earliest of the three markers, if any.
        let bold_at = rest.find("**");
        let code_at = rest.find('`');
        let italic_at = rest.find('*').filter(|&i| !rest[i..].starts_with("**"));

        let next = [bold_at, code_at, italic_at].into_iter().flatten().min();
        let Some(at) = next else {
            spans.push(Span::raw(rest.to_string()));
            break;
        };
        if at > 0 {
            spans.push(Span::raw(rest[..at].to_string()));
        }
        if rest[at..].starts_with("**") {
            if let Some(close) = rest[at + 2..].find("**") {
                let body = &rest[at + 2..at + 2 + close];
                spans.push(Span::styled(
                    body.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                rest = &rest[at + 4 + close..];
                continue;
            }
        }
        if rest[at..].starts_with('`') {
            if let Some(close) = rest[at + 1..].find('`') {
                let body = &rest[at + 1..at + 1 + close];
                spans.push(Span::styled(
                    body.to_string(),
                    Style::default().fg(Color::Green),
                ));
                rest = &rest[at + 2 + close..];
                continue;
            }
        }
        if rest[at..].starts_with('*') {
            if let Some(close) = rest[at + 1..].find('*') {
                let body = &rest[at + 1..at + 1 + close];
                spans.push(Span::styled(
                    body.to_string(),
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
                rest = &rest[at + 2 + close..];
                continue;
            }
        }
        // Marker found but no closer — emit the rest as plain text and stop.
        spans.push(Span::raw(rest[at..].to_string()));
        break;
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    spans
}

fn strip_action_fences(text: &str) -> String {
    // Cheaper than a full markdown parser: scan for ```maestro-action ...
    // ``` and elide. Anything else passes through unchanged.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("```maestro-action") {
        out.push_str(&rest[..start]);
        if let Some(end_rel) = rest[start + 17..].find("```") {
            // skip past the closing fence + trailing newline if any
            let absolute_end = start + 17 + end_rel + 3;
            if absolute_end < rest.len() && rest.as_bytes()[absolute_end] == b'\n' {
                rest = &rest[absolute_end + 1..];
            } else {
                rest = &rest[absolute_end..];
            }
        } else {
            // unclosed — drop everything from start (rare; means the LLM
            // emitted a truncated action block).
            break;
        }
    }
    out.push_str(rest);
    out
}

fn action_card_lines(a: &Action) -> Vec<Line<'static>> {
    let (icon, color) = match a.status {
        Some(ActionStatus::Pending) | None => ("▶", Color::Yellow),
        Some(ActionStatus::Approved) | Some(ActionStatus::Running) => ("◐", Color::Blue),
        Some(ActionStatus::Done) => ("✓", Color::Green),
        Some(ActionStatus::Failed) => ("✗", Color::Red),
        Some(ActionStatus::Rejected) => ("⊘", Color::DarkGray),
    };
    let status_word = match a.status {
        Some(ActionStatus::Pending) | None => "pending — [y]es / [n]o",
        Some(ActionStatus::Approved) => "approved",
        Some(ActionStatus::Running) => "running…",
        Some(ActionStatus::Done) => "done",
        Some(ActionStatus::Failed) => "failed",
        Some(ActionStatus::Rejected) => "rejected",
    };
    let mut out = vec![Line::from(vec![
        Span::raw("    "),
        Span::styled(
            format!("{icon} action · "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(a.label.clone(), Style::default().fg(Color::Cyan)),
    ])];
    out.push(Line::from(vec![
        Span::raw("      "),
        Span::styled(
            format!(
                "status: {status_word}  ·  id: {}",
                &a.id[..a.id.len().min(8)]
            ),
            Style::default().fg(color),
        ),
    ]));
    if let Some(output) = &a.output {
        // Show first ~5 lines of captured output, truncated. The full
        // log lives on disk if the user wants the rest.
        let head: Vec<&str> = output.lines().take(5).collect();
        for hl in head {
            out.push(Line::from(vec![
                Span::raw("      │ "),
                Span::styled(hl.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
        if output.lines().count() > 5 {
            out.push(Line::from(Span::styled(
                "      │ …",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    out
}

fn draw_banner(f: &mut Frame, area: Rect, app: &AppState) {
    let (title_line, meter_line) = if let Some(s) = &app.state {
        // Header line: run id + status + N/M progress + verified chip
        let done = s
            .tasks
            .values()
            .filter(|t| {
                matches!(
                    t.status,
                    TaskStatus::Done
                        | TaskStatus::Failed
                        | TaskStatus::Cancelled
                        | TaskStatus::Skipped
                )
            })
            .count();
        let total = s.tasks.len().max(1);
        let status = run_status_label(&s.status);
        let verified = if s.verified {
            Span::styled(" · verified", Style::default().fg(Color::Green))
        } else {
            Span::raw("")
        };
        let title = Line::from(vec![
            Span::styled(
                format!("maestro · {} ", s.run_id),
                Style::default().fg(Color::Cyan),
            ),
            status,
            Span::styled(
                format!("  {done}/{total}"),
                Style::default().fg(Color::DarkGray),
            ),
            verified,
        ]);

        // Phase 5: token + context_bytes live meter — answers the two
        // bookkeeping questions ("am I burning budget?" and "did the
        // model see what I told it to remember?") at a glance.
        let total_tokens_in: u64 = s.usage.input_tokens;
        let total_tokens_out: u64 = s.usage.output_tokens;
        let context_total: u64 = s.tasks.values().filter_map(|t| t.context_bytes).sum();
        let context_count: usize = s
            .tasks
            .values()
            .filter(|t| t.context_bytes.is_some())
            .count();
        let meter = Line::from(vec![
            Span::styled(s.spec.clone(), Style::default().fg(Color::DarkGray)),
            Span::raw("   "),
            Span::styled(
                format!(
                    "tokens · {} in / {} out",
                    human_count(total_tokens_in),
                    human_count(total_tokens_out)
                ),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("   "),
            Span::styled(
                format!(
                    "context · {} across {} task(s)",
                    human_bytes(context_total),
                    context_count
                ),
                Style::default().fg(Color::Magenta),
            ),
        ]);
        (title, meter)
    } else if let Some(e) = &app.load_error {
        (
            Line::from(Span::styled(
                format!("maestro · could not load run state: {e}"),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
        )
    } else {
        (
            Line::from(Span::styled(
                "maestro · loading…",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
        )
    };
    let p = Paragraph::new(vec![title_line, meter_line])
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(p, area);
}

/// Compact thousands-formatter for token counts: 12345 → "12.3K".
fn human_count(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format!("{:.1}K", n as f64 / 1_000.0);
    }
    format!("{:.1}M", n as f64 / 1_000_000.0)
}

fn draw_body(f: &mut Frame, area: Rect, app: &mut AppState) {
    // Phase 5: when `d` is on, body splits TASKS | detail | diff. Off
    // (default), the body keeps the Phase 1-4 layout TASKS | detail.
    let constraints: Vec<Constraint> = if app.show_diff {
        vec![
            Constraint::Length(32),
            Constraint::Percentage(45),
            Constraint::Min(0),
        ]
    } else {
        vec![Constraint::Length(32), Constraint::Min(0)]
    };
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let tasks = app.ordered_tasks();
    let items: Vec<ListItem> = tasks
        .iter()
        .map(|t| {
            let (icon, color) = status_glyph(&t.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::raw(truncate(&t.id, 24)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" TASKS · {} ", tasks.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Indexed(237))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, chunks[0], &mut app.selected);

    draw_detail(f, chunks[1], app);
    if app.show_diff {
        draw_diff(f, chunks[2], app);
    }
}

fn draw_diff(f: &mut Frame, area: Rect, app: &mut AppState) {
    let block = Block::default().borders(Borders::ALL).title(" diff ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(task) = app.selected_task().cloned() else {
        let p = Paragraph::new("(no task selected)").style(Style::default().fg(Color::DarkGray));
        f.render_widget(p, inner);
        return;
    };
    let workspace = task
        .worktree_path
        .clone()
        .or_else(|| task.workspace_path.clone());
    let Some(ws) = workspace else {
        f.render_widget(
            Paragraph::new("(no workspace recorded for this task)")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    // Fetch + cache on demand. ratatui's render path is sync, so we
    // shell out to git here — fine because git diff against HEAD on a
    // local repo is sub-50ms in practice and we re-cache after.
    let diff_text = app
        .diff_cache
        .entry(task.id.clone())
        .or_insert_with(|| fetch_git_diff(&ws).unwrap_or_else(|| "(no diff vs HEAD)".to_string()))
        .clone();

    let lines: Vec<Line<'static>> = diff_text
        .lines()
        .take((inner.height as usize).saturating_sub(1))
        .map(|l| {
            let style = if l.starts_with('+') && !l.starts_with("+++") {
                Style::default().fg(Color::Green)
            } else if l.starts_with('-') && !l.starts_with("---") {
                Style::default().fg(Color::Red)
            } else if l.starts_with("@@") {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else if l.starts_with("diff ")
                || l.starts_with("index ")
                || l.starts_with("+++")
                || l.starts_with("---")
            {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(l.to_string(), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// Cheap shell-out to `git diff HEAD` for the diff pane. Capped at ~8K
/// chars so the side-panel doesn't suck the whole render budget on a
/// large refactor; the user can `git diff` themselves for the full text.
fn fetch_git_diff(ws: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(ws)
        .args(["diff", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    Some(s.chars().take(8_000).collect())
}

fn draw_detail(f: &mut Frame, area: Rect, app: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" detail ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(task) = app.selected_task() else {
        let p = Paragraph::new("(no task selected)").style(Style::default().fg(Color::DarkGray));
        f.render_widget(p, inner);
        return;
    };

    let duration = task
        .started_at
        .zip(task.ended_at)
        .map(|(s, e)| format!("{}ms", (e - s).num_milliseconds().max(0)))
        .unwrap_or_else(|| "—".to_string());
    let workspace = task
        .worktree_path
        .as_deref()
        .or(task.workspace_path.as_deref())
        .unwrap_or("—");
    let (status_glyph_str, status_color) = status_glyph(&task.status);
    let context_bytes = task
        .context_bytes
        .map(|b| format!("{} ({} slice(s))", human_bytes(b), task.memory_used.len()))
        .unwrap_or_else(|| "—".to_string());

    let mut lines = vec![
        kv_line("id", &task.id),
        kv_line("project", &task.project),
        kv_line("agent", &task.agent),
        Line::from(vec![
            label("status"),
            Span::styled(
                format!("{status_glyph_str} "),
                Style::default().fg(status_color),
            ),
            Span::styled(
                status_label(&task.status),
                Style::default().fg(status_color),
            ),
        ]),
        kv_line("duration", &duration),
        kv_line("workspace", workspace),
        kv_line("context", &context_bytes),
    ];

    if let Some(role) = &task.role {
        lines.push(kv_line("role", role));
    }
    if !task.skills_triggered.is_empty() {
        lines.push(kv_line("skills", &task.skills_triggered.join(", ")));
    }
    if !task.depends_on.is_empty() {
        lines.push(kv_line("depends_on", &task.depends_on.join(", ")));
    }
    if let Some(err) = &task.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "error",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            truncate(err, 4000),
            Style::default().fg(Color::Red),
        )));
    }
    if matches!(task.status, TaskStatus::AwaitingApproval) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "⏵ press 'a' to approve  ·  command: maestro approve {}",
                task.id
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        // Note: 'a' is wired in Phase 2 (slash commands). The hint stays so
        // users discover the affordance even now.
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, inner);
}

fn draw_help_line(f: &mut Frame, area: Rect, app: &AppState) {
    // Three states for the bottom strip:
    //   1. Command mode → `:` prompt + the user's input buffer + cursor.
    //   2. Active flash toast (recent action outcome) → colored line.
    //   3. Default → keybinding hint.
    if app.mode == Mode::Command {
        let line = Line::from(vec![
            Span::styled(
                ":",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.cmd_input.clone()),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    if let Some((msg, kind, at)) = &app.flash {
        if at.elapsed() < Duration::from_secs(6) {
            let color = match kind {
                FlashKind::Info => Color::Cyan,
                FlashKind::Success => Color::Green,
                FlashKind::Error => Color::Red,
            };
            let prefix = match kind {
                FlashKind::Info => "ⓘ",
                FlashKind::Success => "✓",
                FlashKind::Error => "✗",
            };
            f.render_widget(
                Paragraph::new(format!("{prefix} {msg}")).style(Style::default().fg(color)),
                area,
            );
            return;
        }
    }

    let txt = match app.mode {
        Mode::Chat if app.newest_pending_action().is_some() && app.chat_input.is_empty() => {
            "[Enter] send · [y/n] approve / reject pending action · [Esc/Tab] leave chat · [Ctrl-C] quit"
        }
        Mode::Chat => "[Enter] send · [Esc/Tab] leave chat · [Ctrl-C] quit",
        _ => "[j/k] nav · [Tab/i] chat · [a] approve · [d] diff · [c] cancel · [:] command · [?] help · [q] quit",
    };
    f.render_widget(
        Paragraph::new(txt).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    // Center a small modal in the screen.
    let modal = centered_rect(60, 40, area);
    let lines = vec![
        Line::from(Span::styled(
            " maestro chat-tui · phase 5 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  navigation",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    j / ↓        next task"),
        Line::from("    k / ↑        previous task"),
        Line::from("    g / Home     first task"),
        Line::from("    G / End      last task"),
        Line::from("    r            force refresh"),
        Line::from("    ?            toggle this help"),
        Line::from("    q / Esc      quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  quick actions on the selected task",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    a            approve"),
        Line::from("    c            prefill /cancel (then Enter to confirm)"),
        Line::from(""),
        Line::from(Span::styled(
            "  : or / enters command mode",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    /approve [task]   write approval marker (defaults to selected)"),
        Line::from("    /cancel           cancel the run (force-cancels if dead)"),
        Line::from("    /logs [task]      show task log path"),
        Line::from("    /rerun            print the maestro rerun command"),
        Line::from("    /replay           print the maestro runs replay command"),
        Line::from("    /refresh          force state reload"),
        Line::from("    /help · /quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  chat with the orchestrator (Tab or i)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    Enter   send to agent"),
        Line::from("    Esc/Tab leave chat back to nav"),
        Line::from("    thinking trace renders dim above the answer"),
        Line::from(""),
        Line::from(Span::styled(
            "  inline action cards (new in Phase 4)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    y     approve the newest pending action"),
        Line::from("    n     reject it (no execution)"),
        Line::from("    status streams back into the card (running → done/failed)"),
        Line::from(""),
        Line::from(Span::styled(
            "  Phase 5: diff side-panel + meters + markdown",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    d        toggle the diff side-panel (vs HEAD)"),
        Line::from("    banner   live token in/out + context_bytes meter"),
        Line::from("    chat     # heading / - list / `code` / **bold** styled inline"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Indexed(235)))
            .title(" help "),
    );
    f.render_widget(ratatui::widgets::Clear, modal);
    f.render_widget(p, modal);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}

fn label(s: &str) -> Span<'static> {
    Span::styled(
        format!("{s:<11}", s = s),
        Style::default().fg(Color::DarkGray),
    )
}

fn kv_line(k: &str, v: &str) -> Line<'static> {
    Line::from(vec![label(k), Span::raw(v.to_string())])
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn human_bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    if n < 1024 * 1024 {
        return format!("{:.1} KB", n as f64 / 1024.0);
    }
    format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
}

fn status_label(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::AwaitingApproval => "awaiting approval",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Skipped => "skipped",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn status_glyph(s: &TaskStatus) -> (&'static str, Color) {
    match s {
        TaskStatus::Pending => ("○", Color::DarkGray),
        TaskStatus::Running => ("◐", Color::Blue),
        TaskStatus::AwaitingApproval => ("⏸", Color::Yellow),
        TaskStatus::Done => ("✓", Color::Green),
        TaskStatus::Failed => ("✗", Color::Red),
        TaskStatus::Skipped => ("↷", Color::DarkGray),
        TaskStatus::Cancelled => ("⊘", Color::DarkGray),
    }
}

fn run_status_label(s: &RunStatus) -> Span<'static> {
    let (label, color) = match s {
        RunStatus::Running => ("running", Color::Blue),
        RunStatus::Done => ("done", Color::Green),
        RunStatus::Failed => ("failed", Color::Red),
        RunStatus::Cancelled => ("cancelled", Color::DarkGray),
    };
    Span::styled(label, Style::default().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_picks_the_right_unit() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3), "3.0 MB");
    }

    #[test]
    fn truncate_appends_ellipsis_when_over_max() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world!", 8), "hello w…");
    }

    fn fresh_app() -> AppState {
        AppState::new(
            std::path::PathBuf::from("/tmp/nonexistent-run-for-tests"),
            1000,
        )
    }

    #[test]
    fn unknown_command_flashes_error_and_does_not_crash() {
        let mut app = fresh_app();
        run_command(&mut app, "/wat");
        let (msg, kind, _) = app.flash.as_ref().expect("flash must be set");
        assert!(matches!(kind, FlashKind::Error));
        assert!(
            msg.contains("/wat"),
            "error msg should echo the bad command"
        );
    }

    #[test]
    fn refresh_command_resets_the_refresh_clock() {
        let mut app = fresh_app();
        // Push the clock forward so we can observe it being rolled back.
        app.last_refresh = Instant::now();
        run_command(&mut app, "/refresh");
        // last_refresh should now be in the past, forcing the next tick
        // to actually reload state.
        assert!(app.last_refresh.elapsed() > Duration::from_secs(30));
    }

    #[test]
    fn approve_without_target_or_selection_flashes_error() {
        let mut app = fresh_app();
        // No state loaded → no selectable task.
        run_command(&mut app, "/approve");
        let (_, kind, _) = app.flash.as_ref().expect("flash must be set");
        assert!(matches!(kind, FlashKind::Error));
    }

    #[test]
    fn slash_prefix_is_optional() {
        // Vim users naturally type `:approve T_x` (no slash); codex
        // users prefer `/approve T_x`. Both must work.
        let mut a = fresh_app();
        let mut b = fresh_app();
        run_command(&mut a, "approve T_x"); // no slash
        run_command(&mut b, "/approve T_x"); // with slash
                                             // Both should at least produce a flash (success OR error — outcome
                                             // depends on filesystem; here we just check the dispatcher parsed
                                             // them the same way).
        let a_kind = match a.flash.as_ref().unwrap().1 {
            FlashKind::Success => "ok",
            FlashKind::Error => "err",
            FlashKind::Info => "info",
        };
        let b_kind = match b.flash.as_ref().unwrap().1 {
            FlashKind::Success => "ok",
            FlashKind::Error => "err",
            FlashKind::Info => "info",
        };
        assert_eq!(
            a_kind, b_kind,
            "the dispatcher must treat / and bare-name the same"
        );
    }

    #[test]
    fn chat_message_lines_prefixes_role_icon_only_on_first_visual_line() {
        let m = ChatMessage::new(
            ChatRole::Assistant,
            "first line\nsecond line\nthird".to_string(),
        );
        let lines = chat_message_lines(&m, 80);
        assert_eq!(lines.len(), 3, "one line per source line");
        // First line carries the role glyph; subsequent lines indent only.
        let s0 = format!("{:?}", lines[0]);
        let s1 = format!("{:?}", lines[1]);
        assert!(s0.contains("🤖"), "first line must carry assistant icon");
        assert!(!s1.contains("🤖"), "wrapped lines must NOT repeat the icon");
    }

    #[test]
    fn chat_message_lines_renders_thinking_above_answer_in_dim() {
        let mut m = ChatMessage::new(ChatRole::Assistant, "the answer".to_string());
        m.thinking = Some("first thought\nsecond thought".to_string());
        let lines = chat_message_lines(&m, 80);
        // 2 thinking + 1 answer = 3 lines
        assert_eq!(lines.len(), 3);
        let s0 = format!("{:?}", lines[0]);
        let s2 = format!("{:?}", lines[2]);
        assert!(s0.contains("💭"), "thinking lines carry the 💭 glyph");
        assert!(s2.contains("🤖"), "answer line carries the assistant glyph");
    }

    #[test]
    fn drain_stream_events_accumulates_text_into_pending_bubble() {
        let mut app = fresh_app();
        let (tx, rx) = mpsc::channel::<StreamEvent>(8);
        app.chat_rx = Some(rx);
        app.chat_streaming = true;
        // Push two delta events synchronously through the channel
        tx.try_send(StreamEvent::Delta {
            text: "hello ".into(),
        })
        .unwrap();
        tx.try_send(StreamEvent::Delta {
            text: "world".into(),
        })
        .unwrap();
        drain_stream_events(&mut app);
        let (visible, thinking) = app.chat_pending.as_ref().unwrap();
        assert_eq!(visible, "hello world");
        assert!(thinking.is_empty());
        assert!(
            app.chat_streaming,
            "Done not received yet — still streaming"
        );
    }

    #[test]
    fn drain_stream_events_error_clears_streaming_and_flashes() {
        let mut app = fresh_app();
        let (tx, rx) = mpsc::channel::<StreamEvent>(4);
        app.chat_rx = Some(rx);
        app.chat_streaming = true;
        tx.try_send(StreamEvent::Error {
            message: "spawn failed".into(),
        })
        .unwrap();
        drain_stream_events(&mut app);
        assert!(!app.chat_streaming);
        assert!(app.chat_rx.is_none());
        let (msg, kind, _) = app.flash.as_ref().unwrap();
        assert!(matches!(kind, FlashKind::Error));
        assert!(msg.contains("spawn failed"));
    }

    #[test]
    fn strip_action_fences_removes_only_action_blocks() {
        let raw = "Sure, here's the plan.\n\n```maestro-action\nverb: approve\ntask: T_x\n```\n\nDoes that look right?";
        let stripped = strip_action_fences(raw);
        assert!(stripped.contains("Sure, here's the plan."));
        assert!(stripped.contains("Does that look right?"));
        assert!(!stripped.contains("verb: approve"));
        assert!(!stripped.contains("```"));
    }

    #[test]
    fn strip_action_fences_keeps_other_code_blocks_intact() {
        // Only `maestro-action` fences are stripped; regular code blocks
        // remain so the user still sees the agent's snippets.
        let raw = "Here's some code:\n\n```rust\nlet x = 1;\n```\n";
        let stripped = strip_action_fences(raw);
        assert!(stripped.contains("let x = 1;"));
        assert!(stripped.contains("```rust"));
    }

    #[test]
    fn action_card_lines_show_yn_prompt_when_pending() {
        let mut args = std::collections::BTreeMap::new();
        args.insert("task".to_string(), "T_demo".to_string());
        let a = Action {
            id: "act-abc123".into(),
            verb: crate::chat::actions::ActionVerb::Approve,
            args,
            status: Some(ActionStatus::Pending),
            label: "maestro approve T_demo".to_string(),
            output: None,
        };
        let lines = action_card_lines(&a);
        let dump = lines
            .iter()
            .map(|l| format!("{l:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("maestro approve T_demo"));
        assert!(dump.contains("pending"));
        assert!(dump.contains("[y]es") || dump.contains("y]es"));
    }

    #[test]
    fn newest_pending_action_returns_none_on_clean_session() {
        let app = fresh_app();
        assert!(app.newest_pending_action().is_none());
    }

    #[test]
    fn human_count_formats_thousands_and_millions() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(12_300), "12.3K");
        assert_eq!(human_count(2_500_000), "2.5M");
    }

    #[test]
    fn markdown_inline_styles_headings_lists_and_emphasis() {
        // Headings strip the prefix and apply bold.
        let h2 = render_markdown_inline("## A heading", false);
        let dump = format!("{h2:?}");
        assert!(dump.contains("A heading"));
        assert!(dump.contains("BOLD"));
        assert!(!dump.contains("## "), "heading prefix must be consumed");

        // List bullet replaced with • and the rest emphasized.
        let li = render_markdown_inline("- buy **eggs**", false);
        let dump = format!("{li:?}");
        assert!(dump.contains("•"));
        assert!(dump.contains("eggs"));

        // Inline code rendered as a separate styled span.
        let code = render_markdown_inline("Use `cargo test` to run", false);
        let dump = format!("{code:?}");
        assert!(dump.contains("cargo test"));
        assert!(dump.contains("Green"));
    }

    #[test]
    fn markdown_inline_passes_through_inside_code_block() {
        // Inside a fenced code block markdown markers must NOT be styled
        // — e.g. `# include <stdio.h>` is a C preprocessor directive,
        // not a heading.
        let out = render_markdown_inline("# include <stdio.h>", true);
        // Exactly one span (the raw line in DarkGray)
        assert_eq!(out.len(), 1);
        let dump = format!("{out:?}");
        assert!(dump.contains("# include"));
        assert!(dump.contains("DarkGray"));
    }

    #[test]
    fn markdown_inline_passes_plain_text_unchanged() {
        let out = render_markdown_inline("plain old prose", false);
        // Plain text returns a single Raw span — cheap fast-path.
        assert_eq!(out.len(), 1);
        let dump = format!("{out:?}");
        assert!(dump.contains("plain old prose"));
    }

    #[test]
    fn markdown_inline_handles_unclosed_markers_gracefully() {
        // Unclosed `**bold...` should NOT panic / loop — emit the rest
        // as plain text and stop.
        let out = render_markdown_inline("here we go **and never close", false);
        let dump = format!("{out:?}");
        assert!(dump.contains("and never close"));
    }

    #[test]
    fn status_glyphs_cover_every_task_status() {
        // Belt-and-suspenders: any new TaskStatus variant added later must
        // be handled here, otherwise the chat-tui will silently render the
        // missing-arm panic. Exhaustively touch every variant.
        for s in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::AwaitingApproval,
            TaskStatus::Done,
            TaskStatus::Failed,
            TaskStatus::Skipped,
            TaskStatus::Cancelled,
        ] {
            let (g, _) = status_glyph(&s);
            assert!(!g.is_empty());
            assert!(!status_label(&s).is_empty());
        }
    }
}
