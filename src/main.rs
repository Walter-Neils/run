//! win-run — a Windows "Run" style floating launcher for nixpkgs programs.

mod hyprland;
mod index;
mod launcher;
mod nix;

use std::collections::HashMap;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;

const WINDOW_W: f32 = 680.0;
const WINDOW_H: f32 = 440.0;
const APP_ID: &str = "win-run";
const MAX_RESULTS: usize = 50;

/// Messages sent from worker threads back to the UI.
enum Msg {
    Index(Result<Vec<index::Package>, String>),
    Exes {
        attr: String,
        result: Result<Vec<String>, nix::ResolveError>,
    },
}

/// A prompt shown when a package needs unfree/insecure evaluation to continue.
struct ConfirmDialog {
    attr: String,
    message: String,
    unfree: bool,
    insecure: bool,
    /// 0 = Continue, 1 = Cancel.
    selected: usize,
}

enum Mode {
    Search,
    /// The chosen package exposes several executables; ask which one to run.
    ChooseExe {
        attr: String,
        exes: Vec<String>,
    },
}

struct WinRun {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,

    index: Vec<index::Package>,
    index_ready: bool,
    index_error: Option<String>,

    query: String,
    last_query: String,
    results: Vec<usize>,
    selected: usize,
    mode: Mode,

    terminal: bool,
    busy: Option<String>,
    /// True while a selected package is being resolved, so the search UI can be
    /// hidden to prevent picking another package mid-resolution.
    resolving: bool,
    status: Option<String>,

    /// Persisted once the user confirms allowing unfree/insecure evaluation.
    allow_unfree: bool,
    allow_insecure: bool,
    confirm: Option<ConfirmDialog>,

    exe_cache: HashMap<String, Vec<String>>,

    positioned: bool,
    hypr_thread_started: bool,
    should_close: bool,
}

impl WinRun {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let (tx, rx) = unbounded();
        let idx_tx = tx.clone();
        std::thread::spawn(move || {
            let rev = index::nixpkgs_rev();
            let result = index::build_index(&rev);
            let _ = idx_tx.send(Msg::Index(result));
        });

        Self {
            rx,
            tx,
            index: Vec::new(),
            index_ready: false,
            index_error: None,
            query: String::new(),
            last_query: String::new(),
            results: Vec::new(),
            selected: 0,
            mode: Mode::Search,
            terminal: false,
            busy: Some("Loading nixpkgs index…".to_string()),
            resolving: false,
            status: None,
            allow_unfree: false,
            allow_insecure: false,
            confirm: None,
            exe_cache: HashMap::new(),
            positioned: false,
            hypr_thread_started: false,
            should_close: false,
        }
    }

    fn recompute(&mut self) {
        self.last_query = self.query.clone();
        self.results = index::search(&self.index, &self.query, MAX_RESULTS);
        self.selected = 0;
    }

    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Index(Ok(packages)) => {
                    self.index = packages;
                    self.index_ready = true;
                    self.busy = None;
                    self.recompute();
                }
                Msg::Index(Err(e)) => {
                    self.index_error = Some(e);
                    self.busy = None;
                }
                Msg::Exes { attr, result } => {
                    self.busy = None;
                    self.resolving = false;
                    match result {
                        Ok(exes) => {
                            self.exe_cache.insert(attr.clone(), exes.clone());
                            self.after_resolve(attr, exes);
                        }
                        Err(e) if e.needs_confirmation() => {
                            self.confirm = Some(ConfirmDialog {
                                attr,
                                message: e.message,
                                unfree: e.unfree,
                                insecure: e.insecure,
                                selected: 0,
                            });
                        }
                        Err(e) => self.status = Some(e.message),
                    }
                }
            }
        }
    }

    fn allow(&self) -> nix::Allow {
        nix::Allow {
            unfree: self.allow_unfree,
            insecure: self.allow_insecure,
        }
    }

    /// Resolve (build + list) the executables of a package, asynchronously.
    fn choose_attr(&mut self, attr: String) {
        if let Some(exes) = self.exe_cache.get(&attr).cloned() {
            self.after_resolve(attr, exes);
            return;
        }
        self.status = None;
        self.busy = Some(format!("Resolving {attr}…"));
        self.resolving = true;
        let tx = self.tx.clone();
        let allow = self.allow();
        std::thread::spawn(move || {
            let result = nix::resolve_executables(&attr, allow);
            let _ = tx.send(Msg::Exes { attr, result });
        });
    }

    /// Apply the "single executable or executable named like the package" rule.
    fn after_resolve(&mut self, attr: String, exes: Vec<String>) {
        let pname = self
            .index
            .iter()
            .find(|p| p.attr == attr)
            .map(|p| p.pname.clone())
            .unwrap_or_default();
        let tail = attr.rsplit('.').next().unwrap_or(&attr).to_string();

        if exes.len() == 1 {
            self.launch(&attr, &exes[0]);
        } else if let Some(exe) = exes.iter().find(|e| **e == pname || **e == tail).cloned() {
            self.launch(&attr, &exe);
        } else {
            self.mode = Mode::ChooseExe { attr, exes };
            self.query.clear();
            self.last_query.clear();
            self.selected = 0;
        }
    }

    fn launch(&mut self, attr: &str, exe: &str) {
        let allow = self.allow();
        match launcher::launch(attr, exe, self.terminal, allow) {
            Ok(()) => {
                self.status = Some(format!("Launched {exe}"));
                self.busy = None;
                self.should_close = true;
            }
            Err(e) => {
                self.busy = None;
                self.status = Some(e);
            }
        }
    }

    /// Position the window. Hyprland needs a background poll; other
    /// compositors get a best-effort winit position request.
    fn try_position(&mut self, ctx: &egui::Context) {
        if hyprland::is_hyprland() {
            if !self.hypr_thread_started {
                self.hypr_thread_started = true;
                let (w, h) = (WINDOW_W as u32, WINDOW_H as u32);
                std::thread::spawn(move || {
                    for _ in 0..40 {
                        if hyprland::center_window(APP_ID, w, h) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(75));
                    }
                });
            }
            self.positioned = true;
            return;
        }

        if let Some(size) = ctx.input(|i| i.viewport().monitor_size) {
            let pos = egui::pos2(
                (size.x - WINDOW_W).max(0.0) / 2.0,
                (size.y - WINDOW_H).max(0.0) / 2.0,
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            self.positioned = true;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(busy) = &self.busy {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(egui::RichText::new(busy).strong());
            } else if let Some(status) = &self.status {
                ui.label(egui::RichText::new(status).color(egui::Color32::LIGHT_RED));
            } else {
                let hint = match self.mode {
                    Mode::Search => "↑↓ select · Enter run · Ctrl+Enter in terminal · Esc quit",
                    Mode::ChooseExe { .. } => "↑↓ select executable · Enter run · Esc back",
                };
                ui.label(egui::RichText::new(hint).weak());
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.terminal, "Run in terminal");
            });
        });
    }
}

impl eframe::App for WinRun {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump();

        if !self.positioned {
            self.try_position(&ctx);
        }

        if self.should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Consume navigation keys before the text field can swallow them.
        let arrow_up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
        let arrow_down =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let ctrl_enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter));
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        // While the confirmation dialog is open the arrow keys drive it; only
        // then do we consume Left/Right (otherwise they'd move the text caret).
        let dialog_open = self.confirm.is_some();
        let arrow_left = dialog_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft));
        let arrow_right = dialog_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight));

        let mut dialog_action: Option<bool> = None;

        if dialog_open {
            if (arrow_up || arrow_down || arrow_left || arrow_right)
                && let Some(d) = &mut self.confirm
            {
                d.selected = 1 - d.selected;
            }
            if escape {
                dialog_action = Some(false);
            }
            if (enter || ctrl_enter)
                && let Some(d) = &self.confirm
            {
                dialog_action = Some(d.selected == 0);
            }
        } else {
            if escape {
                match self.mode {
                    Mode::ChooseExe { .. } => {
                        self.mode = Mode::Search;
                        self.query.clear();
                        self.last_query.clear();
                        self.selected = 0;
                        self.status = None;
                    }
                    Mode::Search => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                }
            }

            if ctrl_enter {
                self.terminal = true;
            }
        }

        egui::Panel::bottom("win-run-status").show(ui, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });

        let exe_mode = matches!(self.mode, Mode::ChooseExe { .. });
        let busy = self.busy.is_some();
        let mut activate: Option<usize> = None;

        egui::CentralPanel::default().show(ui, |ui| {
            // While a package is being resolved, hide the search UI so another
            // package can't be selected in the meantime.
            if self.resolving {
                let label = self
                    .busy
                    .clone()
                    .unwrap_or_else(|| "Resolving…".to_string());
                ui.add_space(70.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(34.0));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(label).strong());
                });
                return;
            }

            ui.add_space(8.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(if exe_mode {
                        "Filter executables…"
                    } else {
                        "Type to run a nixpkgs program…"
                    })
                    .font(egui::FontId::proportional(20.0))
                    .interactive(!dialog_open)
                    .desired_width(f32::INFINITY),
            );
            // The field should always hold keyboard focus so the user can just
            // start typing.
            if !dialog_open && !response.has_focus() {
                response.request_focus();
            }
            ui.add_space(6.0);
            ui.separator();

            if !exe_mode && self.query != self.last_query {
                self.recompute();
            }

            // Build the list of rows for this frame.
            let items: Vec<(String, String)> = match &self.mode {
                Mode::Search => self
                    .results
                    .iter()
                    .filter_map(|&i| self.index.get(i))
                    .map(|p| (p.attr.clone(), p.description.clone()))
                    .collect(),
                Mode::ChooseExe { exes, .. } => exes
                    .iter()
                    .filter(|e| exe_matches(e, &self.query))
                    .map(|e| (e.clone(), String::new()))
                    .collect(),
            };

            // Arrow-key selection.
            if !dialog_open && !items.is_empty() {
                if arrow_down {
                    self.selected = (self.selected + 1).min(items.len() - 1);
                }
                if arrow_up {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.selected = self.selected.min(items.len() - 1);
            }

            let mut selected = self.selected;
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if !self.index_ready {
                        if let Some(err) = &self.index_error {
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new(format!("Failed to build index:\n{err}"))
                                    .color(egui::Color32::LIGHT_RED),
                            );
                        } else {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Spinner::new().size(24.0));
                                ui.label("Building nixpkgs index…");
                            });
                        }
                        return;
                    }

                    if items.is_empty() {
                        ui.add_space(12.0);
                        let msg = if self.query.trim().is_empty() {
                            if exe_mode {
                                "No executables."
                            } else {
                                "Type a program name to search nixpkgs."
                            }
                        } else {
                            "No matches."
                        };
                        ui.label(egui::RichText::new(msg).weak());
                        return;
                    }

                    for (row, (name, desc)) in items.iter().enumerate() {
                        let is_sel = row == selected;
                        let resp = ui
                            .horizontal(|ui| {
                                let r =
                                    ui.selectable_label(is_sel, egui::RichText::new(name).strong());
                                if !desc.is_empty() {
                                    ui.label(egui::RichText::new(desc).weak());
                                }
                                r
                            })
                            .inner;
                        if resp.hovered() {
                            selected = row;
                        }
                        if resp.clicked() {
                            activate = Some(row);
                        }
                        if is_sel {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                });

            self.selected = selected;

            if (enter || ctrl_enter) && !busy && !dialog_open && !items.is_empty() {
                activate = Some(selected);
            }

            if let Some(row) = activate
                && let Some((name, _)) = items.get(row)
            {
                let name = name.clone();
                if exe_mode {
                    if let Mode::ChooseExe { attr, .. } = &self.mode {
                        let attr = attr.clone();
                        self.launch(&attr, &name);
                    }
                } else {
                    self.choose_attr(name);
                }
            }
        });

        // Confirmation dialog for packages that need unfree/insecure evaluation.
        if let Some(d) = &self.confirm {
            let what = match (d.unfree, d.insecure) {
                (true, true) => "an unfree and insecure license",
                (true, false) => "an unfree license",
                (false, true) => "an insecure (unsafe) package",
                (false, false) => "restricted evaluation",
            };
            let _ = egui::Modal::new(egui::Id::new("win-run-confirm")).show(&ctx, |ui| {
                ui.set_max_width(460.0);
                ui.heading("Allow restricted package?");
                ui.add_space(8.0);
                ui.label(format!("“{}” requires {what}.", d.attr));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&d.message).weak().small());
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.selectable_label(d.selected == 0, "Continue").clicked() {
                        dialog_action = Some(true);
                    }
                    if ui.selectable_label(d.selected == 1, "Cancel").clicked() {
                        dialog_action = Some(false);
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Enter confirm · ←/→ choose · Esc cancel")
                        .weak()
                        .small(),
                );
            });
        }

        if let Some(proceed) = dialog_action
            && let Some(d) = self.confirm.take()
            && proceed
        {
            self.allow_unfree |= d.unfree;
            self.allow_insecure |= d.insecure;
            self.choose_attr(d.attr);
        }

        if self.busy.is_some() || !self.positioned || self.should_close {
            ctx.request_repaint();
        }
    }
}

fn exe_matches(exe: &str, query: &str) -> bool {
    let q = query.trim();
    q.is_empty() || exe.to_lowercase().contains(&q.to_lowercase())
}

fn main() -> eframe::Result<()> {
    // Float + centre the window as soon as it maps (Hyprland).
    hyprland::register_window_rule(APP_ID, WINDOW_W as u32, WINDOW_H as u32);

    let viewport = egui::ViewportBuilder::default()
        .with_title("win-run")
        .with_app_id(APP_ID)
        .with_inner_size([WINDOW_W, WINDOW_H])
        .with_min_inner_size([420.0, 220.0])
        .with_window_level(egui::WindowLevel::AlwaysOnTop);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "win-run",
        options,
        Box::new(|cc| Ok(Box::new(WinRun::new(cc)))),
    )
}
