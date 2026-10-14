//! Interacting with `nix` to resolve the executables contained in a package.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const BIN_DIRS: [&str; 2] = ["bin", "sbin"];

/// Permissions to grant when evaluating a package.
#[derive(Clone, Copy, Default)]
pub struct Allow {
    pub unfree: bool,
    pub insecure: bool,
}

impl Allow {
    pub fn any(&self) -> bool {
        self.unfree || self.insecure
    }
}

/// Failure while realizing a package. A failure that only needs the user to
/// allow unfree/insecure evaluation is recoverable via a confirmation prompt.
pub struct ResolveError {
    pub message: String,
    pub unfree: bool,
    pub insecure: bool,
}

impl ResolveError {
    fn other(message: String) -> Self {
        Self {
            message,
            unfree: false,
            insecure: false,
        }
    }

    /// Whether the user can push past this error by allowing the evaluation.
    pub fn needs_confirmation(&self) -> bool {
        self.unfree || self.insecure
    }

    fn from_stderr(stderr: &str, attr: &str) -> Self {
        let lower = stderr.to_ascii_lowercase();
        let unfree = lower.contains("unfree") || lower.contains("allow_unfree");
        let insecure = lower.contains("insecure") || lower.contains("allow_insecure");

        let detail = license_line(stderr).or_else(|| first_error_line(stderr));
        let message = match detail {
            Some(d) if !d.is_empty() => format!("nix build failed for {attr}: {d}"),
            _ => format!("nix build failed for {attr}"),
        };

        Self {
            message,
            unfree,
            insecure,
        }
    }
}

/// Find the nix line that explains *why* the package was refused, which is
/// more useful than the outer "error:" wrapper.
fn license_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("unfree license")
                || line.contains("insecure")
                || line.contains("allow_unfree")
                || line.contains("allow_insecure")
                || line.contains("refusing to evaluate")
        })
        .map(|s| s.to_string())
}

/// Nix often prints a lone `error:` followed by the message on the next line.
fn first_error_line(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim).collect();
    for (i, line) in lines.iter().enumerate() {
        if *line == "error:" {
            return lines[i + 1..]
                .iter()
                .find(|s| !s.is_empty())
                .map(|s| (*s).to_string());
        }
        if let Some(rest) = line.strip_prefix("error:")
            && !rest.trim().is_empty()
        {
            return Some((*line).to_string());
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    // `metadata` follows symlinks, which is what we want: a symlink into the
    // store pointing at a real executable counts.
    match fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// Realize `nixpkgs#<attr>` and list the executable names found in its
/// `bin/` (and `sbin/`) directories.
pub fn resolve_executables(attr: &str, allow: Allow) -> Result<Vec<String>, ResolveError> {
    let mut cmd = Command::new("nix");
    cmd.arg("build");
    if allow.any() {
        cmd.arg("--impure");
    }
    cmd.args(["--no-link", "--print-out-paths", &format!("nixpkgs#{attr}")]);
    if allow.unfree {
        cmd.env("NIXPKGS_ALLOW_UNFREE", "1");
    }
    if allow.insecure {
        cmd.env("NIXPKGS_ALLOW_INSECURE", "1");
    }

    let out = cmd
        .output()
        .map_err(|e| ResolveError::other(format!("failed to run `nix build`: {e}")))?;

    if !out.status.success() {
        return Err(ResolveError::from_stderr(
            &String::from_utf8_lossy(&out.stderr),
            attr,
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut exes: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        for sub in BIN_DIRS {
            let dir = Path::new(line).join(sub);
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || !is_executable(&path) {
                    continue;
                }
                exes.push(name.to_string());
            }
        }
    }

    exes.sort();
    exes.dedup();

    if exes.is_empty() {
        Err(ResolveError::other(format!(
            "no executables found in bin/ for {attr}"
        )))
    } else {
        Ok(exes)
    }
}
