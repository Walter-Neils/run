//! Spawning the resolved program, either detached or inside a terminal.

use std::env;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::nix::Allow;

/// Launch `exe` from package `attr`, optionally inside a terminal emulator.
pub fn launch(attr: &str, exe: &str, terminal: bool, allow: Allow) -> Result<(), String> {
    if terminal {
        launch_in_terminal(attr, exe, allow)
    } else {
        launch_detached(attr, exe, allow)
    }
}

/// Apply the `NIXPKGS_ALLOW_*` environment variables needed to evaluate
/// unfree/insecure packages. `--impure` is added by the caller right after the
/// sub-command (nix rejects it in the global position).
fn apply_allow(cmd: &mut Command, allow: Allow) {
    if allow.unfree {
        cmd.env("NIXPKGS_ALLOW_UNFREE", "1");
    }
    if allow.insecure {
        cmd.env("NIXPKGS_ALLOW_INSECURE", "1");
    }
}

fn which(program: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Quote a string for POSIX shells using single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Spawn the program fully detached from win-run (no controlling terminal),
/// logging its output so failures are still diagnosable.
fn launch_detached(attr: &str, exe: &str, allow: Allow) -> Result<(), String> {
    let cache = crate::index::cache_dir();
    let _ = std::fs::create_dir_all(&cache);
    let log_path = cache.join("last-run.log");

    let to_log = || -> Stdio {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map(Stdio::from)
            .unwrap_or(Stdio::null())
    };

    let mut cmd = Command::new("setsid");
    cmd.arg("nix");
    cmd.arg("shell");
    if allow.any() {
        cmd.arg("--impure");
    }
    apply_allow(&mut cmd, allow);
    cmd.arg(format!("nixpkgs#{attr}"))
        .arg("-c")
        .arg(exe)
        .stdin(Stdio::null())
        .stdout(to_log())
        .stderr(to_log());

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch {exe}: {e}"))
}

fn launch_in_terminal(attr: &str, exe: &str, allow: Allow) -> Result<(), String> {
    let term = detect_terminal().ok_or_else(|| {
        "no terminal emulator found (set $TERMINAL or install kitty/alacritty/foot)".to_string()
    })?;

    let shell = env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
    let mut env_prefix = String::new();
    if allow.unfree {
        env_prefix.push_str("NIXPKGS_ALLOW_UNFREE=1 ");
    }
    if allow.insecure {
        env_prefix.push_str("NIXPKGS_ALLOW_INSECURE=1 ");
    }
    let impure = if allow.any() { "--impure " } else { "" };
    let run = format!(
        "{env_prefix}nix shell {impure}{} -c {}",
        shell_quote(&format!("nixpkgs#{attr}")),
        shell_quote(exe)
    );
    // Keep the terminal open after the program exits.
    let script = format!(
        "{run}; __st=$?; echo; echo \"[win-run] {exe} exited with status $__st\"; exec {} -l",
        shell_quote(&shell)
    );

    let name = Path::new(&term)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let mut cmd = Command::new("setsid");
    cmd.arg(&term);

    match name {
        "kitty" | "foot" => {
            cmd.arg("sh").arg("-lc").arg(script);
        }
        "wezterm" => {
            cmd.arg("start").arg("--").arg("sh").arg("-lc").arg(script);
        }
        _ => {
            // alacritty, ghostty, konsole, xterm, and most others accept `-e`.
            cmd.arg("-e").arg("sh").arg("-lc").arg(script);
        }
    }

    cmd.stdin(Stdio::null());
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch terminal {name}: {e}"))
}

fn detect_terminal() -> Option<String> {
    if let Ok(term) = env::var("TERMINAL")
        && !term.trim().is_empty()
        && let Some(path) = which(&term)
    {
        return Some(path);
    }
    const CANDIDATES: [&str; 8] = [
        "kitty",
        "ghostty",
        "alacritty",
        "wezterm",
        "foot",
        "konsole",
        "gnome-terminal",
        "xterm",
    ];
    CANDIDATES.iter().find_map(|t| which(t))
}
