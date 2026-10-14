# win-run

A Windows "Run"-style floating launcher for [nixpkgs](https://nixos.org) programs,
built with [`eframe`/`egui`](https://github.com/emilk/egui).

Type a program name, pick a matching package, and it is launched — without a
terminal by default.

## Features

- **Floating, centred pop-up window** (always on top) that always keeps keyboard
  focus in the search box.
- **Search as you type** over the whole nixpkgs attribute set. The index is built
  once with `nix search` and cached under `~/.cache/win-run/`, keyed by the
  `nixpkgs` revision.
- **Executable resolution**: the selected package is realized with
  `nix build`, then its `bin/`/`sbin/` executables are listed.
  - exactly one executable → run it;
  - an executable named like the package (`pname`) → run it;
  - otherwise → a second-level picker lets you choose which executable to run.
- **Terminal or not**: launches detached (no terminal) by default. Press
  `Ctrl+Enter`, or tick **Run in terminal**, to launch inside a terminal emulator
  (auto-detects `$TERMINAL`, `kitty`, `ghostty`, `alacritty`, …).
- **Unfree / insecure packages**: if evaluation is refused, a keyboard-navigable
  confirmation dialog offers to continue, re-running nix with
  `NIXPKGS_ALLOW_UNFREE=1` / `NIXPKGS_ALLOW_INSECURE=1` (and `--impure`).
- **Hyprland integration**: a float+center+size window rule is registered via
  `hyprctl`, and the window is nudged to the exact centre once mapped. Other
  compositors get a best-effort winit position request.

## Key bindings

| Key | Action |
| --- | --- |
| type | filter packages / executables |
| `↑` / `↓` | move selection |
| `Enter` | run selected package / executable |
| `Ctrl+Enter` | run in a terminal |
| `Esc` | back (from the executable picker) / quit |

## Usage

```sh
nix develop        # dev shell with the Rust toolchain and GUI libraries
cargo run          # build and run

# or run the packaged binary
nix run .
```

## Layout

```
src/main.rs      UI, state machine, keyboard handling
src/index.rs     nixpkgs index build/cache + fuzzy search
src/nix.rs       realize a package, list executables, detect restricted errors
src/launcher.rs  spawn detached or in a terminal
src/hyprland.rs  Hyprland window placement
```
