//! Building, caching and fuzzy-searching the nixpkgs package index.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use serde::{Deserialize, Serialize};

/// A single nixpkgs attribute that can be launched.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Package {
    /// Attribute path usable with `nixpkgs#<attr>`, e.g. `python3Packages.black`.
    pub attr: String,
    pub pname: String,
    pub description: String,
}

impl Package {
    /// Last dot-separated component of the attribute path.
    pub fn tail(&self) -> &str {
        self.attr.rsplit('.').next().unwrap_or(&self.attr)
    }
}

#[derive(Serialize, Deserialize)]
struct Cache {
    rev: String,
    packages: Vec<Package>,
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("win-run")
}

fn index_cache_path(rev: &str) -> PathBuf {
    let safe: String = rev
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    cache_dir().join(format!("index-{safe}.json"))
}

/// Resolve the revision of the `nixpkgs` flake so the index can be cached
/// against a specific snapshot.
pub fn nixpkgs_rev() -> String {
    let out = Command::new("nix")
        .args(["flake", "metadata", "nixpkgs", "--json"])
        .output();
    if let Ok(out) = out
        && out.status.success()
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
    {
        if let Some(rev) = v.get("revision").and_then(|r| r.as_str()) {
            return rev.to_string();
        }
        if let Some(nar) = v.pointer("/locked/narHash").and_then(|r| r.as_str()) {
            return nar.to_string();
        }
    }
    "unknown".to_string()
}

fn load_cache(rev: &str) -> Option<Vec<Package>> {
    let data = fs::read(index_cache_path(rev)).ok()?;
    let cache: Cache = serde_json::from_slice(&data).ok()?;
    if cache.rev == rev && !cache.packages.is_empty() {
        Some(cache.packages)
    } else {
        None
    }
}

fn save_cache(rev: &str, packages: &[Package]) {
    let dir = cache_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let cache = Cache {
        rev: rev.to_string(),
        packages: packages.to_vec(),
    };
    if let Ok(data) = serde_json::to_vec(&cache) {
        // Write to a temp file first so a crash can't leave a half-written cache.
        let tmp = dir.join("index.tmp");
        if fs::write(&tmp, &data).is_ok() {
            let _ = fs::rename(tmp, index_cache_path(rev));
        }
    }
}

/// Turn `legacyPackages.x86_64-linux.hello` into `hello`.
fn strip_platform_prefix(key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() > 2 && (parts[0] == "legacyPackages" || parts[0] == "packages") {
        parts[2..].join(".")
    } else {
        key.to_string()
    }
}

/// Load the index from cache or build it by querying `nix search`.
pub fn build_index(rev: &str) -> Result<Vec<Package>, String> {
    if let Some(packages) = load_cache(rev) {
        return Ok(packages);
    }

    let out = Command::new("nix")
        .args(["search", "nixpkgs", "--json", ".*"])
        .output()
        .map_err(|e| format!("failed to run `nix search`: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "`nix search` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let map: HashMap<String, serde_json::Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("failed to parse `nix search` output: {e}"))?;

    let mut packages: Vec<Package> = map
        .into_iter()
        .map(|(key, val)| Package {
            attr: strip_platform_prefix(&key),
            pname: val
                .get("pname")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            description: val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect();

    packages.sort_by(|a, b| a.attr.cmp(&b.attr));
    packages.dedup_by(|a, b| a.attr == b.attr);

    save_cache(rev, &packages);
    Ok(packages)
}

/// Fuzzy search the index, returning package indices ordered best-first.
pub fn search(index: &[Package], query: &str, limit: usize) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let matcher = SkimMatcherV2::default().ignore_case();
    let needle = query.to_lowercase();
    let mut scored: Vec<(i64, usize)> = Vec::new();

    for (i, pkg) in index.iter().enumerate() {
        let tail = pkg.tail();

        let mut best = matcher.fuzzy_match(tail, query);
        if let Some(s) = matcher.fuzzy_match(&pkg.pname, query) {
            best = Some(best.map_or(s, |b| b.max(s)));
        }

        if let Some(mut score) = best {
            if tail.eq_ignore_ascii_case(query) {
                score += 10_000;
            } else if pkg.pname.eq_ignore_ascii_case(query) {
                score += 9_000;
            } else if tail.to_lowercase().starts_with(&needle) {
                score += 4_000;
            } else if pkg.pname.to_lowercase().starts_with(&needle) {
                score += 3_000;
            }
            scored.push((score, i));
        }
    }

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| index[a.1].attr.len().cmp(&index[b.1].attr.len()))
            .then_with(|| index[a.1].attr.cmp(&index[b.1].attr))
    });

    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}
