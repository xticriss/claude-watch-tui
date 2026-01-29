use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::process::{find_claude_processes, get_shell_pid};
use crate::tmux::{get_pane_map, TmuxLocation};

// Historical session limit
const HISTORY_LIMIT: usize = 20;

// Constants
const JSONL_LINES_TO_SCAN: usize = 100;
const RECENTLY_MODIFIED_THRESHOLD_SECS: f32 = 3.0;
const STALE_FILE_AGE_SECS: f32 = 999.0;
const MESSAGE_TRUNCATE_LEN: usize = 100;

/// Local slash commands that don't trigger Claude to think
const LOCAL_COMMANDS: &[&str] = &[
    "/clear", "/compact", "/help", "/config", "/cost", "/doctor",
    "/init", "/login", "/logout", "/memory", "/model", "/permissions",
    "/pr-comments", "/review", "/status", "/terminal-setup", "/vim",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Thinking,
    Processing,
    Waiting,
    Idle,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Thinking => write!(f, "Thinking"),
            SessionStatus::Processing => write!(f, "Processing"),
            SessionStatus::Waiting => write!(f, "Waiting"),
            SessionStatus::Idle => write!(f, "Idle"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub project_name: String,
    pub project_path: String,
    pub status: SessionStatus,
    pub last_message: Option<String>,
    #[serde(skip)]
    pub tmux_location: Option<TmuxLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_target: Option<String>,
    pub cpu_usage: f32,
    /// Seconds since last activity (JSONL modification)
    pub last_activity_secs: u64,
    /// Process ID (for killing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Whether this session is currently running
    pub is_running: bool,
    /// First prompt from sessions-index.json (for historical sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
    /// Message count from sessions-index.json
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
    /// Creation timestamp (ISO format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Full path to the JSONL file (for deletion)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl_path: Option<String>,
}

/// Entry from sessions-index.json
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIndexEntry {
    session_id: String,
    full_path: String,
    first_prompt: Option<String>,
    message_count: u32,
    created: String,
    modified: String,
    project_path: String,
    #[serde(default)]
    is_sidechain: bool,
}

/// Container for sessions-index.json
#[derive(Debug, Deserialize)]
struct SessionIndex {
    #[allow(dead_code)]
    version: u32,
    entries: Vec<SessionIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct JsonlMessage {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    msg_type: Option<String>,
    message: Option<MessageContent>,
}

#[derive(Debug, Deserialize)]
struct MessageContent {
    role: Option<String>,
    content: Option<serde_json::Value>,
}

/// Get all active Claude sessions
pub fn get_sessions() -> Vec<Session> {
    let mut processes = find_claude_processes();
    let pane_map = get_pane_map();

    // Sort processes by PID (descending) for consistent JSONL assignment
    // Higher PIDs with ongoing activity tend to have most recent JSONL
    processes.sort_by(|a, b| b.pid.cmp(&a.pid));

    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return Vec::new(),
    };

    if !claude_dir.exists() {
        return Vec::new();
    }

    // Build dir_name -> project_path map
    let mut project_dirs: HashMap<String, PathBuf> = HashMap::new();
    if let Ok(entries) = fs::read_dir(&claude_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    project_dirs.insert(dir_name.to_string(), path);
                }
            }
        }
    }

    let mut sessions = Vec::new();

    // Track how many processes we've seen per project (for JSONL file assignment)
    let mut project_process_index: HashMap<String, usize> = HashMap::new();

    // Iterate over PROCESSES (not project dirs) to support multiple sessions per directory
    for process in &processes {
        let cwd = match &process.cwd {
            Some(c) => c.to_string_lossy().to_string(),
            None => continue,
        };

        let dir_name = convert_path_to_dir_name(&cwd);

        // Find matching project directory
        let project_dir = match project_dirs.get(&dir_name) {
            Some(p) => p,
            None => continue,
        };

        // Get index for this process (0 = most recent JSONL, 1 = second, etc.)
        let jsonl_index = *project_process_index.get(&dir_name).unwrap_or(&0);
        project_process_index.insert(dir_name.clone(), jsonl_index + 1);

        // Find tmux location for this process
        let tmux_location = get_shell_pid(process.pid)
            .and_then(|shell_pid| pane_map.get(&shell_pid).cloned());

        // Parse the Nth most recent JSONL file
        if let Some(session) = parse_project_session(project_dir, &cwd, tmux_location, process.cpu_usage, jsonl_index, process.pid) {
            sessions.push(session);
        }
    }

    // Sort by tmux location (session:window) for stable order
    sessions.sort_by(|a, b| {
        a.tmux_target.cmp(&b.tmux_target)
    });

    sessions
}

/// Get all sessions (running + historical from sessions-index.json)
pub fn get_all_sessions() -> Vec<Session> {
    // Start with running sessions
    let running_sessions = get_sessions();
    let running_ids: std::collections::HashSet<String> = running_sessions.iter()
        .map(|s| s.id.clone())
        .collect();

    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return running_sessions,
    };

    if !claude_dir.exists() {
        return running_sessions;
    }

    // Collect historical sessions from all sessions-index.json files
    let mut historical: Vec<Session> = Vec::new();

    if let Ok(entries) = fs::read_dir(&claude_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let index_path = path.join("sessions-index.json");
            if !index_path.exists() {
                continue;
            }

            // Parse sessions-index.json
            if let Ok(content) = fs::read_to_string(&index_path) {
                if let Ok(index) = serde_json::from_str::<SessionIndex>(&content) {
                    for entry in index.entries {
                        // Skip sidechains and already-running sessions
                        if entry.is_sidechain || running_ids.contains(&entry.session_id) {
                            continue;
                        }

                        // Calculate age from modified timestamp
                        let last_activity_secs = parse_iso_age(&entry.modified);

                        // Extract project name from path
                        let project_name = entry.project_path
                            .split('/')
                            .filter(|s| !s.is_empty())
                            .last()
                            .unwrap_or("Unknown")
                            .to_string();

                        historical.push(Session {
                            id: entry.session_id,
                            project_name,
                            project_path: entry.project_path,
                            status: SessionStatus::Idle,
                            last_message: entry.first_prompt.clone(),
                            tmux_location: None,
                            tmux_target: None,
                            cpu_usage: 0.0,
                            last_activity_secs,
                            pid: None,
                            is_running: false,
                            first_prompt: entry.first_prompt,
                            message_count: Some(entry.message_count),
                            created_at: Some(entry.created),
                            jsonl_path: Some(entry.full_path),
                        });
                    }
                }
            }
        }
    }

    // Sort historical by recency (most recent first)
    historical.sort_by(|a, b| a.last_activity_secs.cmp(&b.last_activity_secs));

    // Take only the most recent HISTORY_LIMIT
    historical.truncate(HISTORY_LIMIT);

    // Combine: running first, then historical
    let mut all_sessions = running_sessions;
    all_sessions.extend(historical);

    all_sessions
}

/// Parse ISO timestamp and return seconds ago
fn parse_iso_age(iso_str: &str) -> u64 {
    use chrono::{DateTime, Utc};
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso_str) {
        let now = Utc::now();
        let duration = now.signed_duration_since(dt.with_timezone(&Utc));
        duration.num_seconds().max(0) as u64
    } else {
        // Fallback: very old
        999999
    }
}

fn parse_project_session(
    project_dir: &PathBuf,
    project_path: &str,
    tmux_location: Option<TmuxLocation>,
    cpu_usage: f32,
    jsonl_index: usize,
    pid: u32,
) -> Option<Session> {
    // Find JSONL files sorted by modification time (excluding agent-*.jsonl)
    let mut jsonl_files: Vec<_> = fs::read_dir(project_dir).ok()?
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
        .collect();

    jsonl_files.sort_by(|a, b| b.1.cmp(&a.1));

    // Pick the Nth most recent JSONL file
    let (jsonl_path, modified_time) = jsonl_files.get(jsonl_index)?;

    // Check if file was recently modified
    let file_age = std::time::SystemTime::now()
        .duration_since(*modified_time)
        .map(|d| d.as_secs_f32())
        .unwrap_or(STALE_FILE_AGE_SECS);
    let recently_modified = file_age < RECENTLY_MODIFIED_THRESHOLD_SECS;

    // Read last N lines efficiently
    let lines = read_last_lines(jsonl_path, JSONL_LINES_TO_SCAN)?;

    let mut session_id = None;
    let mut last_role = None;
    let mut has_tool_use = false;
    let mut has_tool_result = false;
    let mut last_message = None;
    let mut is_local_command = false;
    let mut is_interrupted = false;

    for line in lines.iter().rev() {
        if let Ok(msg) = serde_json::from_str::<JsonlMessage>(line) {
            if session_id.is_none() {
                session_id = msg.session_id.clone();
            }

            if let Some(ref content) = msg.message {
                if let Some(ref c) = content.content {
                    let has_content = match c {
                        serde_json::Value::String(s) => !s.is_empty(),
                        serde_json::Value::Array(arr) => !arr.is_empty(),
                        _ => false,
                    };

                    if has_content {
                        // Set status info from the most recent message with content
                        if last_role.is_none() {
                            last_role = content.role.clone();
                            has_tool_use = check_content_type(c, "tool_use");
                            has_tool_result = check_content_type(c, "tool_result");
                            is_local_command = check_local_command(c);
                            is_interrupted = check_interrupted(c);
                        }

                        // Keep looking for text until we find some
                        if last_message.is_none() {
                            last_message = extract_text(c);
                        }
                    }
                }
            }

            // Stop when we have all the info we need
            if session_id.is_some() && last_role.is_some() && last_message.is_some() {
                break;
            }
        }
    }

    let session_id = session_id?;

    // Determine status
    let status = determine_status(
        last_role.as_deref(),
        has_tool_use,
        has_tool_result,
        is_local_command,
        is_interrupted,
        recently_modified,
    );

    // Extract project name
    let project_name = project_path
        .split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("Unknown")
        .to_string();

    // Truncate message
    let last_message = last_message.map(|m| {
        if m.chars().count() > MESSAGE_TRUNCATE_LEN {
            format!("{}...", m.chars().take(MESSAGE_TRUNCATE_LEN).collect::<String>())
        } else {
            m
        }
    });

    let tmux_target = tmux_location.as_ref().map(|l| l.to_string());

    Some(Session {
        id: session_id,
        project_name,
        project_path: project_path.to_string(),
        status,
        last_message,
        tmux_location,
        tmux_target,
        cpu_usage,
        last_activity_secs: file_age as u64,
        pid: Some(pid),
        is_running: true,
        first_prompt: None,
        message_count: None,
        created_at: None,
        jsonl_path: None,
    })
}

/// Read the last N lines from a file efficiently
fn read_last_lines(path: &PathBuf, n: usize) -> Option<Vec<String>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();

    if file_size == 0 {
        return Some(Vec::new());
    }

    // For small files, just read everything
    if file_size < 64 * 1024 {
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().flatten().collect();
        let start = lines.len().saturating_sub(n);
        return Some(lines[start..].to_vec());
    }

    // For larger files, read from the end in chunks
    let mut file = file;
    let chunk_size = 32 * 1024u64; // 32KB chunks
    let mut lines = Vec::new();
    let mut pos = file_size;
    let mut remainder = String::new();

    while lines.len() < n && pos > 0 {
        let read_size = chunk_size.min(pos);
        pos = pos.saturating_sub(read_size);

        file.seek(SeekFrom::Start(pos)).ok()?;
        let mut buffer = vec![0u8; read_size as usize];
        std::io::Read::read_exact(&mut file, &mut buffer).ok()?;

        let chunk = String::from_utf8_lossy(&buffer);
        let combined = format!("{}{}", chunk, remainder);

        let mut chunk_lines: Vec<&str> = combined.lines().collect();

        // The first line might be partial (unless we're at the start of the file)
        if pos > 0 && !chunk_lines.is_empty() {
            remainder = chunk_lines.remove(0).to_string();
        } else {
            remainder.clear();
        }

        // Add lines in reverse order (we're reading backwards)
        for line in chunk_lines.into_iter().rev() {
            lines.push(line.to_string());
            if lines.len() >= n {
                break;
            }
        }
    }

    // Include any remaining partial line from the start
    if !remainder.is_empty() && lines.len() < n {
        lines.push(remainder);
    }

    // Reverse to get chronological order
    lines.reverse();
    Some(lines)
}

fn determine_status(
    role: Option<&str>,
    has_tool_use: bool,
    has_tool_result: bool,
    is_local_command: bool,
    is_interrupted: bool,
    recently_modified: bool,
) -> SessionStatus {
    match role {
        Some("assistant") => {
            if has_tool_use {
                if recently_modified {
                    SessionStatus::Processing
                } else {
                    SessionStatus::Waiting
                }
            } else if recently_modified {
                SessionStatus::Processing
            } else {
                SessionStatus::Waiting
            }
        }
        Some("user") => {
            // Interrupted requests and local commands mean session is waiting
            if is_local_command || is_interrupted {
                SessionStatus::Waiting
            } else if has_tool_result {
                if recently_modified {
                    SessionStatus::Thinking
                } else {
                    SessionStatus::Waiting
                }
            } else if recently_modified {
                SessionStatus::Thinking
            } else {
                SessionStatus::Waiting
            }
        }
        _ => {
            if recently_modified {
                SessionStatus::Thinking
            } else {
                SessionStatus::Idle
            }
        }
    }
}

/// Check if content array contains a specific type (tool_use, tool_result, etc.)
fn check_content_type(content: &serde_json::Value, type_name: &str) -> bool {
    if let serde_json::Value::Array(arr) = content {
        arr.iter().any(|item| {
            item.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == type_name)
                .unwrap_or(false)
        })
    } else {
        false
    }
}

/// Check if message indicates an interrupted request (user pressed Escape)
fn check_interrupted(content: &serde_json::Value) -> bool {
    extract_text(content)
        .map(|text| text.contains("[Request interrupted by user]"))
        .unwrap_or(false)
}

fn check_local_command(content: &serde_json::Value) -> bool {
    let text = match extract_text(content) {
        Some(t) => t,
        None => return false,
    };
    let trimmed = text.trim();

    LOCAL_COMMANDS.iter().any(|cmd| {
        trimmed == *cmd || trimmed.starts_with(&format!("{} ", cmd))
    })
}

fn extract_text(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            arr.iter().find_map(|v| {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
        }
        _ => None,
    }
}

/// Convert path to directory name (same logic as agent-sessions)
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

#[allow(dead_code)]
/// Convert directory name back to path
fn convert_dir_name_to_path(dir_name: &str) -> String {
    let name = dir_name.strip_prefix('-').unwrap_or(dir_name);
    let parts: Vec<&str> = name.split('-').collect();

    if parts.is_empty() {
        return String::new();
    }

    // Find "Projects" or similar markers
    let projects_idx = parts.iter().position(|&p| p == "Projects" || p == "Development");

    if let Some(idx) = projects_idx {
        let path_parts = &parts[..=idx];
        let project_parts = &parts[idx + 1..];

        let mut path = String::from("/");
        path.push_str(&path_parts.join("/"));

        if !project_parts.is_empty() {
            path.push('/');
            // Handle hidden folders (double dash)
            let mut in_hidden = false;
            let mut segments: Vec<String> = Vec::new();
            let mut current = String::new();

            for part in project_parts {
                if part.is_empty() {
                    if !current.is_empty() {
                        segments.push(current);
                        current = String::new();
                    }
                    in_hidden = true;
                } else if in_hidden {
                    if current.is_empty() {
                        current = format!(".{}", part);
                    } else {
                        segments.push(current);
                        current = part.to_string();
                    }
                } else if current.is_empty() {
                    current = part.to_string();
                } else {
                    current.push('-');
                    current.push_str(part);
                }
            }
            if !current.is_empty() {
                segments.push(current);
            }
            path.push_str(&segments.join("/"));
        }
        path
    } else {
        format!("/{}", name.replace('-', "/"))
    }
}
