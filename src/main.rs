mod process;
mod session;
mod tmux;
mod ui;

use std::io;
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::Terminal;

use session::Session;

struct App {
    sessions: Vec<Session>,
    selected: usize,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            should_quit: false,
        }
    }

    fn refresh_sessions(&mut self) {
        self.sessions = session::get_sessions();
        // Keep selection in bounds
        if self.selected >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
    }

    fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
        }
    }

    fn select_prev(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.sessions.len() - 1);
        }
    }

    fn go_to_selected(&self) {
        if let Some(session) = self.sessions.get(self.selected) {
            if let Some(ref loc) = session.tmux_location {
                tmux::switch_to_window(loc);
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

    let tick_rate = Duration::from_secs(2);
    let mut last_tick = std::time::Instant::now();

    loop {
        terminal.draw(|f| ui::draw(f, &app.sessions, app.selected))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
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
                        // Number shortcuts 1-9
                        KeyCode::Char(c @ '1'..='9') => {
                            let idx = (c as usize) - ('1' as usize);
                            if idx < app.sessions.len() {
                                app.selected = idx;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh_sessions();
            last_tick = std::time::Instant::now();
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
