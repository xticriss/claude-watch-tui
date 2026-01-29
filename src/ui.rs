use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Padding};

use crate::session::{Session, SessionStatus};
use crate::log_view::{self, LogMessage};

// Rose Pine Moon colors (matching your tmux theme)
const GOLD: Color = Color::Rgb(246, 193, 119);      // #f6c177
#[allow(dead_code)]
const ROSE: Color = Color::Rgb(235, 111, 146);      // #eb6f92
const PINE: Color = Color::Rgb(62, 143, 176);       // #3e8fb0
const FOAM: Color = Color::Rgb(156, 207, 216);      // #9ccfd8
#[allow(dead_code)]
const IRIS: Color = Color::Rgb(196, 167, 231);      // #c4a7e7
const SUBTLE: Color = Color::Rgb(110, 106, 134);    // #6e6a86
const MUTED: Color = Color::Rgb(144, 140, 170);     // #908caa
const TEXT: Color = Color::Rgb(224, 222, 244);      // #e0def4
#[allow(dead_code)]
const SURFACE: Color = Color::Rgb(42, 39, 63);      // #2a273f
const OVERLAY: Color = Color::Rgb(57, 53, 82);      // #393552

pub fn draw(frame: &mut Frame, sessions: &[Session], selected: usize, log_messages: &[LogMessage], view_mode: &str) {
    let area = frame.area();

    // Vertical stack: sessions on top, log below
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(18), // ~6 sessions visible (3 lines each)
            Constraint::Min(5),     // Log takes remaining space
        ])
        .split(area);

    let list_area = main_chunks[0];
    let log_area = main_chunks[1];

    // Left pane: session list
    let title = format!(" Claude ({}) ", view_mode);
    let block = Block::default()
        .title(title)
        .title_style(Style::default().bold().fg(GOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SUBTLE))
        .padding(Padding::horizontal(1));

    let inner = block.inner(list_area);
    frame.render_widget(block, list_area);

    // Right pane: log view
    log_view::render_log(frame, log_area, log_messages);

    if sessions.is_empty() {
        let empty_msg = Paragraph::new("No active sessions")
            .style(Style::default().fg(MUTED))
            .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    // Calculate layout: sessions area + legend + help bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let sessions_area = chunks[0];
    let legend_area = chunks[1];
    let help_area = chunks[2];

    // Compact cards: 2 lines each (project+window, message)
    let card_height = 2u16;
    let visible_cards = (sessions_area.height / card_height).max(1) as usize;

    // Scroll to keep selected visible
    let scroll_offset = if selected >= visible_cards {
        selected - visible_cards + 1
    } else {
        0
    };

    let mut y = sessions_area.y;
    for (i, session) in sessions.iter().enumerate().skip(scroll_offset) {
        if y + card_height > sessions_area.y + sessions_area.height {
            break;
        }

        let card_area = Rect::new(sessions_area.x, y, sessions_area.width, card_height);
        let is_selected = i == selected;
        render_session_card(frame, session, card_area, is_selected, i);
        y += card_height;
    }

    // Legend bar (matches tmux tab icons)
    let legend = Paragraph::new(Line::from(vec![
        Span::styled("↻ ", Style::default().fg(GOLD)),
        Span::styled("work  ", Style::default().fg(SUBTLE)),
        Span::styled("◐ ", Style::default().fg(FOAM)),
        Span::styled("wait  ", Style::default().fg(SUBTLE)),
        Span::styled("✓ ", Style::default().fg(SUBTLE)),
        Span::styled("idle  ", Style::default().fg(SUBTLE)),
        Span::styled("○ ", Style::default().fg(MUTED)),
        Span::styled("hist", Style::default().fg(SUBTLE)),
    ])).alignment(Alignment::Center);
    frame.render_widget(legend, legend_area);

    // Compact help bar
    let help = Paragraph::new(Line::from(vec![
        Span::styled("1-9", Style::default().fg(FOAM)),
        Span::styled(" jump ", Style::default().fg(SUBTLE)),
        Span::styled("j/k", Style::default().fg(FOAM)),
        Span::styled(" nav ", Style::default().fg(SUBTLE)),
        Span::styled("↵/r", Style::default().fg(FOAM)),
        Span::styled(" go ", Style::default().fg(SUBTLE)),
        Span::styled("x", Style::default().fg(FOAM)),
        Span::styled(" kill ", Style::default().fg(SUBTLE)),
        Span::styled("D", Style::default().fg(FOAM)),
        Span::styled(" del ", Style::default().fg(SUBTLE)),
        Span::styled("Tab", Style::default().fg(FOAM)),
        Span::styled(" view ", Style::default().fg(SUBTLE)),
        Span::styled("q", Style::default().fg(FOAM)),
        Span::styled(" quit", Style::default().fg(SUBTLE)),
    ])).alignment(Alignment::Center);
    frame.render_widget(help, help_area);
}

/// Format seconds into human-readable relative time
fn format_relative_time(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn render_session_card(frame: &mut Frame, session: &Session, area: Rect, selected: bool, index: usize) {
    // Historical sessions get a different icon
    let (status_icon, status_color) = if !session.is_running {
        ("○", MUTED)  // Historical/not running
    } else {
        match session.status {
            SessionStatus::Thinking => ("↻", GOLD),      // working/thinking
            SessionStatus::Processing => ("↻", PINE),    // working/processing
            SessionStatus::Waiting => ("◐", FOAM),       // waiting for input
            SessionStatus::Idle => ("✓", SUBTLE),        // idle/done
        }
    };

    let bg_color = if selected { OVERLAY } else { Color::Reset };

    // For selected: simple solid background fill
    if selected {
        let fill = " ".repeat(area.width as usize);
        let fill_style = Style::default().bg(bg_color);
        for row in 0..area.height {
            frame.render_widget(
                Paragraph::new(fill.clone()).style(fill_style),
                Rect::new(area.x, area.y + row, area.width, 1)
            );
        }
    }

    let inner = area;

    let width = inner.width as usize;

    // Line 1: [index] status icon + project name + [window#] + relative time
    if inner.height >= 1 {
        let line1_area = Rect::new(inner.x, inner.y, inner.width, 1);

        // Dim historical sessions slightly
        let text_color = if session.is_running { TEXT } else { MUTED };
        let name_style = if selected {
            Style::default().bold().fg(text_color)
        } else {
            Style::default().fg(text_color)
        };

        // Index number (1-9, then nothing)
        let index_str = if index < 9 {
            format!("{}", index + 1)
        } else {
            " ".to_string()
        };

        // Window number badge (compact)
        let window_badge = session.tmux_location.as_ref()
            .map(|l| format!(":{}", l.window_index))
            .unwrap_or_default();

        // Relative time
        let time_str = format_relative_time(session.last_activity_secs);
        let time_width = time_str.len() + 1;

        // Truncate project name if too long
        let badge_len = window_badge.chars().count();
        let max_name_len = width.saturating_sub(6 + time_width + badge_len);
        let name = if session.project_name.len() > max_name_len {
            format!("{}…", &session.project_name[..max_name_len.saturating_sub(1)])
        } else {
            session.project_name.clone()
        };

        // Calculate padding for right-aligned time
        let used_width = 4 + name.chars().count() + badge_len;
        let padding = width.saturating_sub(used_width + time_width);

        let line1 = Line::from(vec![
            Span::styled(format!("{} ", index_str), Style::default().fg(SUBTLE)),
            Span::styled(format!("{} ", status_icon), Style::default().fg(status_color)),
            Span::styled(name, name_style),
            Span::styled(window_badge, Style::default().fg(SUBTLE)),
            Span::styled(" ".repeat(padding), Style::default()),
            Span::styled(time_str, Style::default().fg(SUBTLE)),
        ]);
        frame.render_widget(Paragraph::new(line1), line1_area);
    }

    // Line 2: last message preview (or first_prompt for historical)
    if inner.height >= 2 {
        let line2_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);

        // For historical sessions, prefer first_prompt; for running, use last_message
        let message = if !session.is_running {
            session.first_prompt.as_deref()
                .or(session.last_message.as_deref())
                .unwrap_or("—")
        } else {
            session.last_message.as_deref().unwrap_or("—")
        };

        // Clean up message: remove newlines, collapse whitespace
        let clean_msg: String = message
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        let max_len = width.saturating_sub(6);
        let truncated = if clean_msg.chars().count() > max_len {
            format!("    {}…", clean_msg.chars().take(max_len.saturating_sub(1)).collect::<String>())
        } else {
            format!("    {}", clean_msg)
        };

        // Dim historical session messages
        let msg_color = if session.is_running { MUTED } else { SUBTLE };
        let line2 = Paragraph::new(truncated).style(Style::default().fg(msg_color));
        frame.render_widget(line2, line2_area);
    }
}
