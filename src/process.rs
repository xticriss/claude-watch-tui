use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use sysinfo::{ProcessRefreshKind, RefreshKind, System, Pid};

// Constants
const MAX_PARENT_WALK_DEPTH: usize = 10;
const KNOWN_SHELLS: &[&str] = &["zsh", "bash", "fish", "sh", "dash", "ksh", "tcsh"];

/// Represents a running Claude Code process
#[derive(Debug, Clone)]
pub struct ClaudeProcess {
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub cpu_usage: f32,
}

// Cache System instance to avoid expensive re-initialization
static SYSTEM: Mutex<Option<System>> = Mutex::new(None);

/// Find all running Claude Code processes, excluding sub-agents
/// Returns processes with their CPU usage for status determination
pub fn find_claude_processes() -> Vec<ClaudeProcess> {
    let mut system_guard = SYSTEM.lock().unwrap();

    let system = system_guard.get_or_insert_with(|| {
        System::new_with_specifics(
            RefreshKind::new().with_processes(
                ProcessRefreshKind::new()
                    .with_cmd(sysinfo::UpdateKind::Always)
                    .with_cwd(sysinfo::UpdateKind::Always)
                    .with_cpu()
            )
        )
    });

    // Refresh process data
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new()
            .with_cmd(sysinfo::UpdateKind::Always)
            .with_cwd(sysinfo::UpdateKind::Always)
            .with_cpu()
    );

    // First pass: collect all Claude PIDs
    let claude_pids: HashSet<Pid> = system.processes()
        .iter()
        .filter(|(_, proc)| is_claude_process(proc))
        .map(|(pid, _)| *pid)
        .collect();

    // Second pass: collect non-subagent Claude processes
    system.processes()
        .iter()
        .filter(|(_, proc)| is_claude_process(proc))
        .filter(|(_, proc)| {
            // Exclude if parent is also Claude (sub-agent)
            if let Some(ppid) = proc.parent() {
                if claude_pids.contains(&ppid) {
                    return false;
                }
                // Exclude Zed's external agent (claude-code-acp)
                if let Some(parent_proc) = system.process(ppid) {
                    let parent_cmd: String = parent_proc.cmd()
                        .iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if parent_cmd.contains("claude-code-acp") {
                        return false;
                    }
                }
            }
            true
        })
        .map(|(pid, proc)| ClaudeProcess {
            pid: pid.as_u32(),
            cwd: proc.cwd().map(|p| p.to_path_buf()),
            cpu_usage: proc.cpu_usage(),
        })
        .collect()
}

fn is_claude_process(proc: &sysinfo::Process) -> bool {
    // Skip our own monitoring app
    let name = proc.name().to_string_lossy();
    if name.contains("claude-watch") {
        return false;
    }

    proc.cmd().first()
        .map(|arg| {
            let s = arg.to_string_lossy().to_lowercase();
            s == "claude" || s.ends_with("/claude")
        })
        .unwrap_or(false)
}

/// Get the parent shell PID for a Claude process by walking up the process tree
/// Uses the cached System instance for efficiency
pub fn get_shell_pid(pid: u32) -> Option<u32> {
    let system_guard = SYSTEM.lock().unwrap();
    let system = system_guard.as_ref()?;

    let mut current_pid = Pid::from_u32(pid);

    for _ in 0..MAX_PARENT_WALK_DEPTH {
        let proc = system.process(current_pid)?;
        let name = proc.name().to_string_lossy();

        // Check if this is a known shell (exact match or path ending)
        let name_lower = name.to_lowercase();
        if KNOWN_SHELLS.iter().any(|shell| {
            name_lower == *shell || name_lower.ends_with(&format!("/{}", shell))
        }) {
            return Some(current_pid.as_u32());
        }

        // Move to parent
        current_pid = proc.parent()?;
    }
    None
}
