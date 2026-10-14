//! Hyprland integration.
//!
//! Wayland explicitly does not let clients position their own windows, and
//! Hyprland ignores client-requested positions. To keep the Run window in the
//! middle of the screen we:
//!   1. register a float+center+size window rule for our app_id (so the window
//!      never appears tiled), and
//!   2. after the window has mapped, nudge it to the exact centre.
//!
//! The config on this machine is Lua-based, so the primary path uses
//! `hyprctl eval`; classic `hyprctl keyword windowrulev2` is kept as a fallback
//! for a stock Hyprland.

use std::env;
use std::process::Command;

use serde_json::Value;

pub fn is_hyprland() -> bool {
    env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn hyprctl(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("hyprctl").args(args).output().ok()?;
    if out.status.success() {
        Some(out.stdout)
    } else {
        None
    }
}

/// Register a window rule so the window floats, is sized correctly, and is
/// centred as soon as it maps.
pub fn register_window_rule(app_id: &str, width: u32, height: u32) {
    if !is_hyprland() {
        return;
    }

    let lua = format!(
        r#"hl.window_rule({{ match = {{ class = "{app_id}" }}, float = true, center = true, size = "{width} {height}" }})"#
    );
    let _ = hyprctl(&["eval", &lua]);

    // Stock-Hyprland fallbacks.
    let re = format!("class:^({app_id})$");
    let _ = hyprctl(&["keyword", "windowrulev2", &format!("float,{re}")]);
    let _ = hyprctl(&["keyword", "windowrulev2", &format!("center,{re}")]);
    let _ = hyprctl(&[
        "keyword",
        "windowrulev2",
        &format!("size {width} {height},{re}"),
    ]);
}

/// Try to centre the window with the given app_id. Returns `true` once the
/// window has been found (and moved).
pub fn center_window(app_id: &str, width: u32, height: u32) -> bool {
    if !is_hyprland() {
        return false;
    }

    let Some(clients_bytes) = hyprctl(&["-j", "clients"]) else {
        return false;
    };
    let Ok(clients) = serde_json::from_slice::<Value>(&clients_bytes) else {
        return false;
    };
    let Some(list) = clients.as_array() else {
        return false;
    };

    let class = |c: &Value, key: &str| c.get(key).and_then(|v| v.as_str()) == Some(app_id);
    let Some(win) = list
        .iter()
        .find(|c| class(c, "class") || class(c, "initialClass"))
    else {
        return false;
    };

    let Some(addr) = win.get("address").and_then(|v| v.as_str()) else {
        return false;
    };
    let floating = win
        .get("floating")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let (mx, my, mw, mh) = monitor_geometry(win).unwrap_or((0, 0, 3840, 2160));
    let x = mx + (mw - width as i64) / 2;
    let y = my + (mh - height as i64) / 2;

    if !floating {
        let _ = hyprctl(&["dispatch", "setfloating", &format!("address:{addr}")]);
    }
    let _ = hyprctl(&[
        "dispatch",
        "resizewindowpixel",
        &format!("exact {width} {height},address:{addr}"),
    ]);
    let _ = hyprctl(&[
        "dispatch",
        "movewindowpixel",
        &format!("exact {x} {y},address:{addr}"),
    ]);

    true
}

/// Find the monitor containing the window's centre and return its logical
/// geometry as `(x, y, width, height)`.
fn monitor_geometry(win: &Value) -> Option<(i64, i64, i64, i64)> {
    let at = win.get("at")?.as_array()?;
    let size = win.get("size")?.as_array()?;
    let cx = at.first()?.as_i64()? + size.first()?.as_i64()? / 2;
    let cy = at.get(1)?.as_i64()? + size.get(1)?.as_i64()? / 2;

    let bytes = hyprctl(&["-j", "monitors"])?;
    let monitors: Value = serde_json::from_slice(&bytes).ok()?;

    for m in monitors.as_array()? {
        let x = m.get("x")?.as_i64()?;
        let y = m.get("y")?.as_i64()?;
        let w = m.get("width")?.as_i64()?;
        let h = m.get("height")?.as_i64()?;
        if cx >= x && cx < x + w && cy >= y && cy < y + h {
            return Some((x, y, w, h));
        }
    }
    None
}
