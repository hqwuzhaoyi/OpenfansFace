use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupReport {
    pub previous_daemon_pid: Option<u32>,
    pub killed_worker_pids: Vec<u32>,
    pub removed_sockets: Vec<String>,
    pub free_vram_bytes: i64,
    pub vram_floor_ok: bool,
}

/// Allowed operator patterns for cleanup.
const OPERATOR_PATTERNS: &[&str] = &[
    "watermark_worker",
    "processing_faces",
    "processing_compute",
    "services.gpu_inference",
];

/// Prohibited patterns: NEVER touch or terminate these under any circumstance.
const PROHIBITED_PATTERNS: &[&str] = &["indextts", "tts"];

/// Check if a command line string is an allowed worker process and not prohibited.
pub fn is_target_worker_process(args: &str) -> bool {
    let lower = args.to_lowercase();
    for prohibited in PROHIBITED_PATTERNS {
        if lower.contains(prohibited) {
            return false;
        }
    }
    for pattern in OPERATOR_PATTERNS {
        if lower.contains(pattern) {
            return true;
        }
    }
    false
}

/// Send a signal to a process using rustix::process.
fn send_signal(pid: u32, sig: rustix::process::Signal) -> bool {
    if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
        rustix::process::kill_process(p, sig).is_ok()
    } else {
        false
    }
}

/// Test if process with given PID is alive (kill(pid, 0)).
pub fn is_process_alive(pid: u32) -> bool {
    if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
        rustix::process::test_kill_process(p).is_ok()
    } else {
        false
    }
}

/// Terminate a process with SIGTERM, wait up to max_wait, then SIGKILL if still running.
pub fn terminate_process(pid: u32, max_wait: Duration) -> bool {
    if !is_process_alive(pid) {
        return true;
    }

    send_signal(pid, rustix::process::Signal::TERM);

    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(50);
    while start.elapsed() < max_wait {
        if !is_process_alive(pid) {
            return true;
        }
        thread::sleep(poll_interval);
    }

    // Still running, escalate to SIGKILL
    send_signal(pid, rustix::process::Signal::KILL);
    thread::sleep(Duration::from_millis(100));
    !is_process_alive(pid)
}

/// Acquire the daemon PID lockfile and terminate any previously running daemon instance.
pub fn check_and_acquire_pid_lock(runtime_dir: &Path) -> (Option<u32>, PathBuf) {
    let _ = fs::create_dir_all(runtime_dir);
    let pid_file = runtime_dir.join("face-agent.pid");
    let current_pid = std::process::id();
    let mut previous_pid = None;

    if pid_file.exists() {
        if let Ok(content) = fs::read_to_string(&pid_file) {
            if let Ok(old_pid) = content.trim().parse::<u32>() {
                if old_pid != current_pid && is_process_alive(old_pid) {
                    eprintln!("Found prior running face-agent daemon (PID {}), terminating...", old_pid);
                    terminate_process(old_pid, Duration::from_millis(1500));
                    previous_pid = Some(old_pid);
                }
            }
        }
    }

    let _ = fs::write(&pid_file, current_pid.to_string());
    (previous_pid, pid_file)
}

/// Release the daemon PID lockfile.
pub fn release_pid_lock(pid_file: &Path) {
    if pid_file.exists() {
        let current_pid = std::process::id();
        if let Ok(content) = fs::read_to_string(pid_file) {
            if content.trim() == current_pid.to_string() {
                let _ = fs::remove_file(pid_file);
            }
        }
    }
}

/// Find orphaned operator worker processes matching signatures belonging to current user.
pub fn scan_orphaned_worker_pids() -> Vec<u32> {
    let current_pid = std::process::id();
    let current_user = std::env::var("USER").unwrap_or_default();

    let output = match Command::new("ps").args(["-A", "-o", "pid,user,args"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => return Vec::new(),
    };

    let mut target_pids = Vec::new();

    for line in output.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let pid_str = parts[0];
        let user_str = parts[1];
        let args = parts[2..].join(" ");

        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };

        if pid == current_pid {
            continue;
        }

        // Only scan processes of current user if USER is known
        if !current_user.is_empty() && user_str != current_user {
            continue;
        }

        if is_target_worker_process(&args) {
            target_pids.push(pid);
        }
    }

    target_pids
}

/// Clean stale Unix domain sockets.
pub fn clean_stale_sockets(socket_paths: &[&str]) -> Vec<String> {
    let mut cleaned = Vec::new();
    for &sock in socket_paths {
        let path = Path::new(sock);
        if path.exists() {
            if fs::remove_file(path).is_ok() {
                cleaned.push(sock.to_string());
            }
        }
    }
    cleaned
}

/// Perform startup orphan process cleanup, socket cleaning, and VRAM floor validation.
pub fn cleanup_orphans_on_startup(runtime_dir: Option<&Path>) -> CleanupReport {
    let default_dir = dirs_fallback();
    let dir = runtime_dir.unwrap_or(&default_dir);

    // 1. PID lock & prior daemon termination
    let (previous_daemon_pid, _) = check_and_acquire_pid_lock(dir);

    // 2. Scan and terminate orphaned Python workers
    let candidate_pids = scan_orphaned_worker_pids();
    let mut killed_worker_pids = Vec::new();
    for pid in candidate_pids {
        eprintln!("Terminating orphaned operator worker (PID {})...", pid);
        if terminate_process(pid, Duration::from_millis(1500)) {
            killed_worker_pids.push(pid);
        }
    }

    // 3. Clean stale sockets
    let removed_sockets = clean_stale_sockets(&["/tmp/face-gpu.sock", "/tmp/face-inference.sock"]);

    // 4. Verify free VRAM floor (>= 600 MiB = 629,145,600 bytes)
    let telem = super::probe::probe_telemetry(None);
    let min_vram_floor = 600 * 1024 * 1024; // 600 MiB
    let vram_floor_ok = telem.free_vram_bytes >= min_vram_floor || telem.free_ram_bytes >= min_vram_floor;

    CleanupReport {
        previous_daemon_pid,
        killed_worker_pids,
        removed_sockets,
        free_vram_bytes: telem.free_vram_bytes,
        vram_floor_ok,
    }
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".openfans")
    } else {
        PathBuf::from("/tmp/.openfans")
    }
}
