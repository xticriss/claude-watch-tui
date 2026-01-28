use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct TmuxLocation {
    pub session: String,
    pub window_index: u32,
    #[allow(dead_code)]
    pub window_name: String,
}

impl std::fmt::Display for TmuxLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.session, self.window_index)
    }
}

/// Get mapping of shell PID -> tmux location
pub fn get_pane_map() -> HashMap<u32, TmuxLocation> {
    let mut map = HashMap::new();

    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_pid}:#{session_name}:#{window_index}:#{window_name}"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(4, ':').collect();
                if parts.len() == 4 {
                    if let Ok(pid) = parts[0].parse::<u32>() {
                        if let Ok(window_index) = parts[2].parse::<u32>() {
                            map.insert(pid, TmuxLocation {
                                session: parts[1].to_string(),
                                window_index,
                                window_name: parts[3].to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    map
}

/// Switch to a specific tmux window
pub fn switch_to_window(location: &TmuxLocation) {
    let target = format!("{}:{}", location.session, location.window_index);
    let _ = Command::new("tmux")
        .args(["select-window", "-t", &target])
        .status();
}

