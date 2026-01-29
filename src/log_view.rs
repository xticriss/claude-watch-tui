use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::SystemTime;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

// Rose Pine Moon colors
const GOLD: Color = Color::Rgb(246, 193, 119);
const FOAM: Color = Color::Rgb(156, 207, 216);
const SUBTLE: Color = Color::Rgb(110, 106, 134);
const MUTED: Color = Color::Rgb(144, 140, 170);
const TEXT: Color = Color::Rgb(224, 222, 244);

const MAX_MESSAGES: usize = 50;
const MAX_LINES_TO_SCAN: usize = 500;

#[derive(Debug, Clone)]
pub struct LogMessage {
    pub role: String,
    pub content: String,
}

/// Get the mtime of the most recent JSONL file for a project
pub fn get_log_mtime(project_dir: &str) -> Option<SystemTime> {
    let claude_dir = dirs::home_dir()?.join(".claude").join("projects");
    let dir_name = convert_path_to_dir_name(project_dir);
    let project_path = claude_dir.join(&dir_name);
    let jsonl_path = find_most_recent_jsonl(&project_path)?;
    fs::metadata(&jsonl_path).and_then(|m| m.modified()).ok()
}

/// Parse JSONL file and extract clean messages (user/assistant text only)
pub fn parse_log_messages(project_dir: &str) -> Vec<LogMessage> {
    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return Vec::new(),
    };

    // Convert project path to dir name
    let dir_name = convert_path_to_dir_name(project_dir);
    let project_path = claude_dir.join(&dir_name);

    if !project_path.exists() {
        return Vec::new();
    }

    // Find most recent JSONL file
    let jsonl_path = match find_most_recent_jsonl(&project_path) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // Read and parse messages
    parse_jsonl_messages(&jsonl_path)
}

fn find_most_recent_jsonl(project_dir: &PathBuf) -> Option<PathBuf> {
    std::fs::read_dir(project_dir).ok()?
        .flatten()
        .filter(|e| {
            let path = e.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            path.extension().map(|ext| ext == "jsonl").unwrap_or(false)
                && !name.starts_with("agent-")
        })
        .filter_map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((e.path(), modified))
        })
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

fn parse_jsonl_messages(path: &PathBuf) -> Vec<LogMessage> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().flatten().collect();

    // Take last N lines for efficiency
    let start = lines.len().saturating_sub(MAX_LINES_TO_SCAN);
    let mut messages = Vec::new();

    for line in lines.into_iter().skip(start) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(msg) = extract_message(&json) {
                messages.push(msg);
            }
        }
    }

    // Keep only recent messages
    if messages.len() > MAX_MESSAGES {
        messages.drain(0..messages.len() - MAX_MESSAGES);
    }

    messages
}

fn extract_message(json: &serde_json::Value) -> Option<LogMessage> {
    let message = json.get("message")?;
    let role = message.get("role")?.as_str()?;

    // Only include user and assistant messages
    if role != "user" && role != "assistant" {
        return None;
    }

    let content = message.get("content")?;
    let text = extract_text_content(content)?;

    // Skip empty or tool-only messages
    if text.trim().is_empty() {
        return None;
    }

    Some(LogMessage {
        role: role.to_string(),
        content: text,
    })
}

fn extract_text_content(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let texts: Vec<String> = arr.iter()
                .filter_map(|item| {
                    let item_type = item.get("type")?.as_str()?;
                    if item_type == "text" {
                        item.get("text")?.as_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

fn convert_path_to_dir_name(path: &str) -> String {
    let path = path.strip_prefix('/').unwrap_or(path);
    let mut result = String::from("-");
    let mut chars = path.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '/' => {
                if chars.peek() == Some(&'.') {
                    result.push('-');
                    result.push('-');
                    chars.next();
                } else {
                    result.push('-');
                }
            }
            _ => result.push(c),
        }
    }
    result
}

/// Render the log view panel
pub fn render_log(frame: &mut Frame, area: Rect, messages: &[LogMessage]) {
    let block = Block::default()
        .title(" Log ")
        .title_style(Style::default().fg(GOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SUBTLE));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if messages.is_empty() {
        let empty = Paragraph::new("No messages yet")
            .style(Style::default().fg(MUTED))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    // Build text with role prefixes - newest first (reverse order)
    let mut lines: Vec<Line> = Vec::new();

    for msg in messages.iter().rev() {
        let (prefix, color) = match msg.role.as_str() {
            "user" => ("› ", FOAM),
            "assistant" => ("  ", TEXT),
            _ => ("  ", MUTED),
        };

        // Wrap long messages
        for (i, line) in msg.content.lines().enumerate() {
            let line_prefix = if i == 0 { prefix } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(line_prefix, Style::default().fg(color)),
                Span::styled(line.to_string(), Style::default().fg(if msg.role == "user" { color } else { TEXT })),
            ]));
        }
        lines.push(Line::from("")); // Spacing between messages
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}
