mod process;
mod session;
mod tmux;
mod ui;
mod log_view;

use std::io;
use std::time::{Duration, SystemTime};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::Terminal;

use session::Session;
use log_view::LogMessage;

struct App {
    sessions: Vec<Session>,
    selected: usize,
    should_quit: bool,
    log_messages: Vec<LogMessage>,
    last_log_mtime: Option<SystemTime>,
}

impl App {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            should_quit: false,
            log_messages: Vec::new(),
            last_log_mtime: None,
        }
    }

    fn refresh_sessions(&mut self) {
        self.sessions = session::get_sessions();
        // Keep selection in bounds
        if self.selected >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
        // Refresh log for selected session
        self.refresh_log();
    }

    fn refresh_log(&mut self) {
        self.refresh_log_if_changed(false);
    }

    fn refresh_log_if_changed(&mut self, check_mtime: bool) {
        if let Some(session) = self.sessions.get(self.selected) {
            // Check if file changed (skip expensive parse if unchanged)
            if check_mtime {
                let current_mtime = log_view::get_log_mtime(&session.project_path);
                if current_mtime == self.last_log_mtime {
                    return; // No change, skip parsing
                }
                self.last_log_mtime = current_mtime;
            } else {
                self.last_log_mtime = log_view::get_log_mtime(&session.project_path);
            }
            self.log_messages = log_view::parse_log_messages(&session.project_path);
        } else {
            self.log_messages.clear();
            self.last_log_mtime = None;
        }
    }

    fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
            self.refresh_log();
        }
    }

    fn select_prev(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.sessions.len() - 1);
            self.refresh_log();
        }
    }

    fn go_to_selected(&self) {
        if let Some(session) = self.sessions.get(self.selected) {
            if let Some(ref loc) = session.tmux_location {
                tmux::switch_to_window(loc);
            }
        }
    }

    fn kill_selected(&mut self) {
        if let Some(session) = self.sessions.get(self.selected) {
            if let Some(pid) = session.pid {
                // Send SIGTERM to gracefully terminate
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                // Refresh after kill
                self.refresh_sessions();
            }
        }
    }
}

fn main() -> io::Result<()> {
    // Check for --list flag
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list" || a == "-l") {
        let sessions = session::get_sessions();
        println!("{}", serde_json::to_string_pretty(&sessions).unwrap_or_default());
        return Ok(());
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run
    let mut app = App::new();
    app.refresh_sessions();

    // Split refresh rates: sessions heavy (2s), log light (500ms)
    let session_tick_rate = Duration::from_secs(2);
    let log_tick_rate = Duration::from_millis(500);
    let mut last_session_tick = std::time::Instant::now();
    let mut last_log_tick = std::time::Instant::now();

    loop {
        terminal.draw(|f| ui::draw(f, &app.sessions, app.selected, &app.log_messages))?;

        let timeout = log_tick_rate.saturating_sub(last_log_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
                        KeyCode::Enter => {
                            app.go_to_selected();
                            app.should_quit = true;
                        }
                        KeyCode::Char('r') => app.refresh_sessions(),
                        KeyCode::Char('x') => app.kill_selected(),
                        // Number shortcuts 1-9
                        KeyCode::Char(c @ '1'..='9') => {
                            let idx = (c as usize) - ('1' as usize);
                            if idx < app.sessions.len() {
                                app.selected = idx;
                                app.refresh_log();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Refresh sessions every 2s (heavy - process detection)
        if last_session_tick.elapsed() >= session_tick_rate {
            app.refresh_sessions();
            last_session_tick = std::time::Instant::now();
        }

        // Refresh log every 500ms (light - only if file changed)
        if last_log_tick.elapsed() >= log_tick_rate {
            app.refresh_log_if_changed(true);
            last_log_tick = std::time::Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
