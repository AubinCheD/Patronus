//! La fenêtre : tâches à gauche, boîte de résultats ou éditeur au centre, barre d'état en bas.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Color32, Key, RichText, ScrollArea, TextEdit};
use patronus_core::engine::Engine;
use patronus_core::exec::Context;
use patronus_core::run::{Run, Status, now};
use patronus_core::schedule::describe;
use patronus_core::task::{ClaudeAction, FileWatch, Jour, Script, Shell, Task, Trigger, Worktree};

use crate::shortcuts::{Binding, Command, Shortcuts};
use crate::state::{self, State};
use crate::theme::{Palette, ThemeChoice};

#[derive(Clone, Copy, PartialEq)]
enum View {
    Inbox,
    Editor,
}

/// Une tâche en cours d'édition. `tools` est le texte saisi pour la liste des outils.
struct Editor {
    id: Option<String>,
    task: Task,
    tools: String,
    error: Option<String>,
    confirm_delete: bool,
}

impl Editor {
    fn new(id: Option<String>, task: Task) -> Editor {
        let tools = task.action.claude.as_ref().map(|c| c.outils.join(", ")).unwrap_or_default();
        Editor { id, task, tools, error: None, confirm_delete: false }
    }

    /// La tâche telle qu'elle sera enregistrée.
    fn built(&self) -> Task {
        let mut t = self.task.clone();
        if let Some(c) = &mut t.action.claude {
            c.outils =
                self.tools.split([',', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        }
        t
    }
}

struct Toast {
    text: String,
    color: Color32,
    since: Instant,
}

/// Ce que l'utilisateur a demandé pendant le dessin, appliqué ensuite (le dessin emprunte l'état).
enum Act {
    Run(String),
    Stop(String),
    StopAll,
    Edit(String),
    NewTask,
    Toggle(String, bool),
    SelectRun(String),
    Inbox,
    Command(Command),
}

pub struct App {
    engine: Engine,
    data_dir: PathBuf,
    state: State,
    shortcuts: Shortcuts,
    palette: Palette,
    view: View,
    selected_task: Option<String>,
    selected_run: Option<String>,
    editor: Option<Editor>,
    filter_task: Option<String>,
    unread_only: bool,
    show_archived: bool,
    show_settings: bool,
    show_problems: bool,
    rebinding: Option<Command>,
    palette_open: bool,
    palette_query: String,
    toasts: Vec<Toast>,
    /// Journal affiché : exécution, texte, date de lecture (relu chaque seconde si elle tourne).
    journal: Option<(String, String, Instant)>,
    capture: Option<(String, Instant, bool)>,
    acts: Vec<Act>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, data_dir: PathBuf, state: State, mut problems: Vec<String>) -> App {
        let (shortcuts, sp) = Shortcuts::from_saved(&state.raccourcis);
        problems.extend(sp);
        let mut engine = Engine::open(&data_dir, now());
        engine.retention_days = state.retention_jours;
        engine.problems.splice(0..0, problems);
        crate::theme::apply(&cc.egui_ctx, state.theme);
        let palette = crate::theme::current(&cc.egui_ctx);
        let mut app = App {
            engine,
            data_dir,
            state,
            shortcuts,
            palette,
            view: View::Inbox,
            selected_task: None,
            selected_run: None,
            editor: None,
            filter_task: None,
            unread_only: false,
            show_archived: false,
            show_settings: false,
            show_problems: false,
            rebinding: None,
            palette_open: false,
            palette_query: String::new(),
            toasts: Vec::new(),
            journal: None,
            capture: std::env::var("PATRONUS_CAPTURE").ok().map(|p| (p, Instant::now(), false)),
            acts: Vec::new(),
        };
        app.selected_task = app.engine.loaded.first().map(|l| l.id.clone());
        app.selected_run = app.engine.runs.first().map(|r| r.id.clone());
        // Essais : la vue à montrer dans la capture.
        match std::env::var("PATRONUS_CAPTURE_VUE").as_deref() {
            Ok("editeur") => {
                if let Some(id) = app.selected_task.clone() {
                    app.open_editor(&id);
                } else {
                    app.new_task();
                }
            }
            Ok("reglages") => app.show_settings = true,
            Ok("palette") => app.palette_open = true,
            _ => {}
        }
        app
    }

    fn save_state(&mut self) {
        self.state.raccourcis = self.shortcuts.to_saved();
        if let Err(e) = state::save(&self.data_dir, &self.state) {
            self.engine.problems.push(format!("Enregistrement des réglages impossible : {e}"));
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, since: Instant::now() });
    }

    fn open_editor(&mut self, id: &str) {
        if let Some(t) = self.engine.task(id) {
            self.editor = Some(Editor::new(Some(id.to_string()), t.clone()));
            self.view = View::Editor;
            self.selected_task = Some(id.to_string());
        } else if let Some(l) = self.engine.loaded.iter().find(|l| l.id == id) {
            let why = l.task.as_ref().err().cloned().unwrap_or_default();
            self.toast(format!("{} : {why} — corriger le fichier {}", l.id, l.path.display()), self.palette.danger);
        }
    }

    fn new_task(&mut self) {
        self.editor = Some(Editor::new(None, Task::new_script("Nouvelle tâche")));
        self.view = View::Editor;
    }

    fn apply(&mut self, act: Act) {
        let t = now();
        match act {
            Act::Run(id) => {
                let queued = self.engine.is_running(&id);
                self.engine.launch(&id, Context { trigger: "à la main".into(), files: vec![] }, t);
                if queued {
                    self.toast("Déjà en cours : relancée à la fin.", self.palette.warn);
                }
                self.view = View::Inbox;
                if let Some(r) = self.engine.runs.first() {
                    self.selected_run = Some(r.id.clone());
                }
            }
            Act::Stop(id) => self.engine.stop(&id),
            Act::StopAll => {
                let n = self.engine.running_count();
                self.engine.stop_all();
                self.toast(format!("Arrêt demandé pour {n} exécution(s)."), self.palette.warn);
            }
            Act::Edit(id) => self.open_editor(&id),
            Act::NewTask => self.new_task(),
            Act::Toggle(id, active) => {
                if let Some(mut task) = self.engine.task(&id).cloned() {
                    task.active = active;
                    if let Err(e) = self.engine.save_task(Some(&id), &task, t) {
                        self.toast(format!("Enregistrement impossible : {e}"), self.palette.danger);
                    }
                }
            }
            Act::SelectRun(id) => {
                self.selected_run = Some(id.clone());
                self.engine.mark_read(&id, true);
                self.view = View::Inbox;
            }
            Act::Inbox => self.view = View::Inbox,
            Act::Command(c) => self.command(c),
        }
    }

    fn command(&mut self, c: Command) {
        match c {
            Command::NewTask => self.new_task(),
            Command::Inbox => self.view = View::Inbox,
            Command::RunSelected => {
                if let Some(id) = self.selected_task.clone() {
                    self.apply(Act::Run(id));
                }
            }
            Command::StopAll => self.apply(Act::StopAll),
            Command::Palette => {
                self.palette_open = !self.palette_open;
                self.palette_query.clear();
            }
            Command::Settings => self.show_settings = !self.show_settings,
            Command::NextUnread => {
                let next = self
                    .visible_runs()
                    .into_iter()
                    .find(|r| !r.lu && r.statut != Status::EnCours)
                    .map(|r| r.id.clone());
                match next {
                    Some(id) => self.apply(Act::SelectRun(id)),
                    None => self.toast("Aucun résultat non lu.", self.palette.text_dim),
                }
            }
        }
    }

    fn visible_runs(&self) -> Vec<&Run> {
        self.engine
            .runs
            .iter()
            .filter(|r| self.filter_task.as_ref().is_none_or(|f| &r.tache == f))
            .filter(|r| !self.unread_only || !r.lu)
            .filter(|r| self.show_archived || !r.archive)
            .collect()
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if let Some(cmd) = self.rebinding {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, mods)) = pressed {
                if key == Key::Escape {
                    self.rebinding = None;
                } else {
                    self.shortcuts.set(cmd, Some(Binding::new(mods, key)));
                    self.rebinding = None;
                    self.save_state();
                }
            }
            return;
        }
        let pressed: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        for (key, mods) in pressed {
            if key == Key::Escape && self.palette_open {
                self.palette_open = false;
                continue;
            }
            // Une touche sans modificateur sert d'abord à la saisie, sauf les touches de fonction.
            let typing = ctx.memory(|m| m.focused().is_some());
            let plain = !mods.ctrl
                && !mods.command
                && !mods.alt
                && !matches!(key, Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6);
            if typing && plain {
                continue;
            }
            if let Some(c) = self.shortcuts.command_for(key, mods) {
                self.command(c);
            }
        }
    }

    // --- Panneaux ---------------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Patronus").strong().color(p.accent));
            ui.separator();
            let unread = self.engine.unread_count();
            let inbox = if unread > 0 {
                format!("Boîte de résultats ({unread})")
            } else {
                "Boîte de résultats".to_string()
            };
            if ui
                .selectable_label(self.view == View::Inbox, inbox)
                .on_hover_text(self.shortcuts.text(Command::Inbox))
                .clicked()
            {
                self.acts.push(Act::Inbox);
            }
            if ui.button("+ Nouvelle tâche").on_hover_text(self.shortcuts.text(Command::NewTask)).clicked() {
                self.acts.push(Act::NewTask);
            }
            let running = self.engine.running_count();
            if ui
                .add_enabled(running > 0, egui::Button::new("■ Tout arrêter"))
                .on_hover_text(self.shortcuts.text(Command::StopAll))
                .clicked()
            {
                self.acts.push(Act::StopAll);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Réglages").on_hover_text(self.shortcuts.text(Command::Settings)).clicked() {
                    self.show_settings = !self.show_settings;
                }
                if ui.button("Commandes…").on_hover_text(self.shortcuts.text(Command::Palette)).clicked() {
                    self.palette_open = true;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette.clone();
        ui.horizontal(|ui| {
            let running = self.engine.running_count();
            if running > 0 {
                ui.spinner();
                ui.label(RichText::new(format!("{running} en cours")).color(p.running));
                ui.separator();
            }
            let next = self
                .engine
                .loaded
                .iter()
                .filter_map(|l| {
                    self.engine
                        .next_run(&l.id)
                        .map(|at| (at, l.task.as_ref().map(|t| t.nom.clone()).unwrap_or_default()))
                })
                .min_by_key(|(at, _)| *at);
            match next {
                Some((at, nom)) => {
                    ui.label(RichText::new(format!("Prochaine : {nom}, {}", fmt_when(at))).color(p.text_dim))
                }
                None => ui.label(RichText::new("Aucune échéance planifiée").color(p.text_dim)),
            };
            let n = self.engine.problems.len();
            if n > 0 {
                ui.separator();
                if ui.link(RichText::new(format!("⚠ {n} problème(s)")).color(p.warn)).clicked() {
                    self.show_problems = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(self.data_dir.display().to_string()).small().color(p.text_dim))
                    .on_hover_text("Dossier des données (tâches, exécutions, réglages)");
            });
        });
    }

    fn task_list(&mut self, ui: &mut egui::Ui) {
        let p = self.palette.clone();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Tâches");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+").on_hover_text("Nouvelle tâche").clicked() {
                    self.acts.push(Act::NewTask);
                }
            });
        });
        ui.separator();
        if self.engine.loaded.is_empty() {
            ui.label(RichText::new("Aucune tâche pour l'instant.").color(p.text_dim));
            if ui.button("Créer une tâche").clicked() {
                self.acts.push(Act::NewTask);
            }
            return;
        }
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for l in &self.engine.loaded {
                let id = &l.id;
                let selected = self.selected_task.as_deref() == Some(id.as_str());
                let frame = egui::Frame::new().inner_margin(6.0).corner_radius(4.0).fill(if selected {
                    p.selection
                } else {
                    Color32::TRANSPARENT
                });
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match &l.task {
                            Err(why) => {
                                ui.label(RichText::new(id).strong());
                                ui.label(RichText::new(format!("illisible : {why}")).color(p.danger).small());
                            }
                            Ok(task) => {
                                ui.horizontal(|ui| {
                                    let mut active = task.active;
                                    if ui.checkbox(&mut active, "").on_hover_text("Active (sinon en pause)").changed() {
                                        self.acts.push(Act::Toggle(id.clone(), active));
                                    }
                                    let name = RichText::new(&task.nom).strong();
                                    ui.label(if task.active { name } else { name.color(p.text_dim) });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if self.engine.is_running(id) {
                                            if ui.small_button("■").on_hover_text("Arrêter").clicked() {
                                                self.acts.push(Act::Stop(id.clone()));
                                            }
                                            ui.spinner();
                                        } else if ui.small_button("▶").on_hover_text("Lancer maintenant").clicked() {
                                            self.acts.push(Act::Run(id.clone()));
                                        }
                                        if ui.small_button("✏").on_hover_text("Modifier").clicked() {
                                            self.acts.push(Act::Edit(id.clone()));
                                        }
                                    });
                                });
                                let problems = task.validate();
                                if !problems.is_empty() {
                                    ui.label(RichText::new(format!("⚠ {}", problems[0])).color(p.warn).small());
                                }
                                let triggers: Vec<String> = task.declencheurs.iter().map(describe).collect();
                                let triggers =
                                    if triggers.is_empty() { "à la main".to_string() } else { triggers.join(" ; ") };
                                ui.label(RichText::new(triggers).small().color(p.text_dim));
                                ui.horizontal(|ui| {
                                    if let Some(r) = self.engine.last_run(id) {
                                        dot(ui, p.status(r.statut), 3.5);
                                        ui.label(
                                            RichText::new(format!("{} {}", r.statut.label(), fmt_when(r.debut)))
                                                .small()
                                                .color(p.text_dim),
                                        );
                                    }
                                    if let Some(at) = self.engine.next_run(id) {
                                        ui.label(
                                            RichText::new(format!("· prochaine {}", fmt_when(at)))
                                                .small()
                                                .color(p.text_dim),
                                        );
                                    }
                                    if self.engine.is_pending(id) {
                                        ui.label(RichText::new("· relance en attente").small().color(p.warn));
                                    }
                                });
                            }
                        }
                    })
                    .response;
                let resp = resp.interact(egui::Sense::click());
                if resp.clicked() {
                    self.selected_task = Some(id.clone());
                    self.filter_task = Some(id.clone());
                    self.acts.push(Act::Inbox);
                }
                if resp.double_clicked() {
                    self.acts.push(Act::Edit(id.clone()));
                }
            }
        });
    }

    fn inbox(&mut self, ui: &mut egui::Ui) {
        let p = self.palette.clone();
        egui::Panel::left("runs").resizable(true).default_size(380.0).min_size(260.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Boîte de résultats");
            });
            ui.horizontal_wrapped(|ui| {
                let label = self
                    .filter_task
                    .as_ref()
                    .map(|f| self.engine.task(f).map(|t| t.nom.clone()).unwrap_or_else(|| f.clone()))
                    .unwrap_or_else(|| "Toutes les tâches".into());
                egui::ComboBox::from_id_salt("filter").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_task, None, "Toutes les tâches");
                    for l in &self.engine.loaded {
                        let name = l.task.as_ref().map(|t| t.nom.clone()).unwrap_or_else(|_| l.id.clone());
                        ui.selectable_value(&mut self.filter_task, Some(l.id.clone()), name);
                    }
                });
                ui.checkbox(&mut self.unread_only, "Non lus");
                ui.checkbox(&mut self.show_archived, "Archivés");
            });
            ui.separator();
            let runs: Vec<Run> = self.visible_runs().into_iter().cloned().collect();
            if runs.is_empty() {
                ui.label(
                    RichText::new("Rien pour l'instant. Lancez une tâche (▶) ou attendez son échéance.")
                        .color(p.text_dim),
                );
            }
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for r in &runs {
                    let selected = self.selected_run.as_deref() == Some(r.id.as_str());
                    let resp = egui::Frame::new()
                        .inner_margin(6.0)
                        .corner_radius(4.0)
                        .fill(if selected { p.selection } else { Color32::TRANSPARENT })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if r.statut == Status::EnCours {
                                    ui.spinner();
                                } else {
                                    dot(ui, p.status(r.statut), 5.0);
                                }
                                let name = RichText::new(&r.tache_nom);
                                ui.label(if r.lu { name } else { name.strong() });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(fmt_when(r.debut)).small().color(p.text_dim));
                                });
                            });
                            ui.label(
                                RichText::new(format!("{} · {}", r.statut.label(), r.declencheur))
                                    .small()
                                    .color(p.status(r.statut)),
                            );
                            let first = r.resume.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                            if !first.is_empty() {
                                ui.add(egui::Label::new(RichText::new(first).small().color(p.text_dim)).truncate());
                            }
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        self.acts.push(Act::SelectRun(r.id.clone()));
                    }
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let run = self.selected_run.as_ref().and_then(|id| self.engine.runs.iter().find(|r| &r.id == id)).cloned();
            match run {
                None => {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Choisissez un résultat à gauche.").color(p.text_dim))
                    });
                }
                Some(r) => self.run_detail(ui, &r),
            }
        });
    }

    fn run_detail(&mut self, ui: &mut egui::Ui, r: &Run) {
        let p = self.palette.clone();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading(&r.tache_nom);
            ui.label(RichText::new(r.statut.label()).strong().color(p.status(r.statut)));
        });
        let mut facts = vec![format!("{} · {}", r.declencheur, fmt_when(r.debut))];
        if let Some(s) = r.duration_secs() {
            facts.push(format!("durée {}", fmt_duration(s)));
        }
        if let Some(c) = r.code {
            facts.push(format!("code {c}"));
        }
        if let Some(c) = &r.cout {
            facts.push(format!("{} + {} tokens · {:.2} $ équivalent API", c.tokens_entree, c.tokens_sortie, c.usd));
        }
        ui.label(RichText::new(facts.join(" · ")).color(p.text_dim));
        ui.horizontal_wrapped(|ui| {
            if r.statut == Status::EnCours {
                if ui.button("■ Arrêter").clicked() {
                    self.acts.push(Act::Stop(r.tache.clone()));
                }
            } else {
                if ui.button("▶ Relancer").clicked() {
                    self.acts.push(Act::Run(r.tache.clone()));
                }
                if r.lu && ui.button("Marquer non lu").clicked() {
                    self.engine.mark_read(&r.id, false);
                }
                if !r.archive && ui.button("Archiver").clicked() {
                    self.engine.archive(&r.id);
                }
            }
            if ui.button("Ouvrir le dossier").on_hover_text("Journal et fichiers de l'exécution").clicked() {
                open_path(&self.engine.store.run_dir(&r.id));
            }
            if let Some(wt) = &r.worktree
                && ui.button("Ouvrir le worktree").on_hover_text(wt.display().to_string()).clicked()
            {
                open_path(wt);
            }
            if ui.button("Modifier la tâche").clicked() {
                self.acts.push(Act::Edit(r.tache.clone()));
            }
        });
        ui.separator();
        if !r.resume.is_empty() {
            ui.label(RichText::new("Résumé").strong());
            ui.add(egui::Label::new(&r.resume).wrap());
            ui.add_space(6.0);
        }
        // Journal, relu chaque seconde tant que l'exécution tourne.
        let stale = match &self.journal {
            Some((id, _, at)) => id != &r.id || (r.statut == Status::EnCours && at.elapsed() > Duration::from_secs(1)),
            None => true,
        };
        if stale {
            self.journal = Some((r.id.clone(), self.engine.store.read_journal(&r.id), Instant::now()));
        }
        let diff = self.engine.store.read_diff(&r.id);
        if let Some(diff) = &diff {
            egui::CollapsingHeader::new(RichText::new("Diff proposé").strong()).default_open(true).show(ui, |ui| {
                ScrollArea::vertical().id_salt("diff").max_height(240.0).show(ui, |ui| {
                    for line in diff.lines() {
                        let color = if line.starts_with('+') && !line.starts_with("+++") {
                            p.ok
                        } else if line.starts_with('-') && !line.starts_with("---") {
                            p.danger
                        } else {
                            p.text_dim
                        };
                        ui.label(RichText::new(line).monospace().color(color));
                    }
                });
            });
        }
        ui.label(RichText::new("Journal").strong());
        let text = self.journal.as_ref().map(|j| j.1.clone()).unwrap_or_default();
        ScrollArea::both().id_salt("journal").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            if text.is_empty() {
                ui.label(RichText::new("(journal vide)").color(p.text_dim));
            }
            ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).selectable(true));
        });
    }

    fn editor_view(&mut self, ui: &mut egui::Ui) {
        let p = self.palette.clone();
        let Some(mut ed) = self.editor.take() else {
            self.view = View::Inbox;
            return;
        };
        let mut save = false;
        let mut close = false;
        let mut delete = false;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading(if ed.id.is_some() { "Modifier la tâche" } else { "Nouvelle tâche" });
            if let Some(id) = &ed.id {
                ui.label(RichText::new(format!("Fichier : taches\\{id}.json")).small().color(p.text_dim));
            }
            ui.add_space(6.0);
            egui::Grid::new("general").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Nom");
                ui.add(TextEdit::singleline(&mut ed.task.nom).desired_width(360.0));
                ui.end_row();
                ui.label("Dépôt");
                let mut depot = ed.task.depot.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
                if ui
                    .add(TextEdit::singleline(&mut depot).desired_width(360.0).hint_text("facultatif : C:\\dev\\contoso-api"))
                    .changed()
                {
                    ed.task.depot = (!depot.trim().is_empty()).then(|| PathBuf::from(depot.trim()));
                }
                ui.end_row();
                ui.label("Active");
                ui.checkbox(&mut ed.task.active, "part à ses échéances (sinon : en pause, à la main seulement)");
                ui.end_row();
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Déclencheurs").strong());
            ui.label(RichText::new("Le lancement à la main est toujours possible.").small().color(p.text_dim));
            let mut remove = None;
            for (i, tr) in ed.task.declencheurs.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        match tr {
                            Trigger::Heure { heure, jours, rattrapage } => {
                                ui.label("À");
                                ui.add(TextEdit::singleline(heure).desired_width(50.0).hint_text("08:00"));
                                ui.label("le");
                                for j in Jour::ALL {
                                    let mut on = jours.contains(&j);
                                    if ui.toggle_value(&mut on, j.label()).changed() {
                                        if on {
                                            jours.push(j);
                                            jours.sort();
                                        } else {
                                            jours.retain(|x| *x != j);
                                        }
                                    }
                                }
                                if jours.is_empty() {
                                    ui.label(RichText::new("(tous les jours)").small().color(p.text_dim));
                                }
                                ui.checkbox(rattrapage, "rattraper si manquée")
                                    .on_hover_text("Si Patronus était fermé à l'heure dite, lancer à l'ouverture (sinon : seulement signalé)");
                            }
                            Trigger::Intervalle { toutes_les_minutes } => {
                                ui.label("Toutes les");
                                ui.add(egui::DragValue::new(toutes_les_minutes).range(1..=1440));
                                ui.label("minutes (comptées depuis minuit)");
                            }
                            Trigger::Fichiers { fichiers } => {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Fichier modifié dans");
                                        let mut d = fichiers.dossier.display().to_string();
                                        if ui.add(TextEdit::singleline(&mut d).desired_width(300.0).hint_text("C:\\dev\\northwind")).changed() {
                                            fichiers.dossier = PathBuf::from(d.trim());
                                        }
                                    });
                                    patterns_edit(ui, "Inclure", &mut fichiers.inclure, "docs/**, *.toml (vide : tout)");
                                    patterns_edit(ui, "Exclure", &mut fichiers.exclure, "facultatif");
                                });
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.small_button("✖").on_hover_text("Retirer ce déclencheur").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                });
            }
            if let Some(i) = remove {
                ed.task.declencheurs.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button("+ Heure").clicked() {
                    ed.task.declencheurs.push(Trigger::Heure { heure: "08:00".into(), jours: vec![], rattrapage: false });
                }
                if ui.button("+ Intervalle").clicked() {
                    ed.task.declencheurs.push(Trigger::Intervalle { toutes_les_minutes: 60 });
                }
                if ui.button("+ Fichier modifié").clicked() {
                    let dossier = ed.task.depot.clone().unwrap_or_default();
                    ed.task.declencheurs.push(Trigger::Fichiers { fichiers: FileWatch { dossier, inclure: vec![], exclure: vec![] } });
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Action").strong());
            let is_claude = ed.task.action.claude.is_some();
            ui.horizontal(|ui| {
                if ui.radio(!is_claude, "Script").clicked() && is_claude {
                    ed.task.action.claude = None;
                    ed.task.action.worktree = None;
                    ed.task.action.script = Some(Script::default());
                }
                if ui.radio(is_claude, "Session Claude non interactive").clicked() && !is_claude {
                    ed.task.action.script = None;
                    ed.task.action.claude = Some(ClaudeAction { budget_usd: Some(1.0), tours_max: Some(30), ..Default::default() });
                }
            });
            if let Some(s) = &mut ed.task.action.script {
                egui::Grid::new("script").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Shell");
                    ui.horizontal(|ui| {
                        for sh in Shell::ALL {
                            ui.selectable_value(&mut s.shell, sh, sh.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Commande");
                    ui.add(
                        TextEdit::multiline(&mut s.commande)
                            .code_editor()
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .hint_text("git -C C:\\dev\\contoso-api log --since=yesterday --oneline"),
                    );
                    ui.end_row();
                    ui.label("Dossier");
                    let mut d = s.dossier.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
                    if ui.add(TextEdit::singleline(&mut d).desired_width(360.0).hint_text("facultatif : le dépôt, sinon le dossier personnel")).changed() {
                        s.dossier = (!d.trim().is_empty()).then(|| PathBuf::from(d.trim()));
                    }
                    ui.end_row();
                });
                ui.label(
                    RichText::new("Le script reçoit PATRONUS_TACHE, PATRONUS_DECLENCHEUR et PATRONUS_FICHIERS (fichiers modifiés).")
                        .small()
                        .color(p.text_dim),
                );
            }
            if let Some(c) = &mut ed.task.action.claude {
                egui::Grid::new("claude").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Consigne");
                    ui.add(TextEdit::multiline(&mut c.consigne).desired_rows(4).desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("Outils permis");
                    ui.add(TextEdit::singleline(&mut ed.tools).desired_width(f32::INFINITY).hint_text("vide : lecture seule (Read, Grep, Glob)"));
                    ui.end_row();
                    ui.label("Tours max");
                    let mut tours = c.tours_max.unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut tours).range(0..=500)).on_hover_text("0 : sans limite").changed() {
                        c.tours_max = (tours > 0).then_some(tours);
                    }
                    ui.end_row();
                    ui.label("Budget ($, équivalent API)");
                    let mut budget = c.budget_usd.unwrap_or(0.0);
                    if ui.add(egui::DragValue::new(&mut budget).range(0.0..=100.0).speed(0.05).max_decimals(2)).on_hover_text("Obligatoire : la session s'arrête au-delà").changed() {
                        c.budget_usd = (budget > 0.0).then_some(budget);
                    }
                    ui.end_row();
                    ui.label("Worktree");
                    let mut use_wt = ed.task.action.worktree.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut use_wt, "dans un worktree dédié de la branche").changed() {
                            ed.task.action.worktree = use_wt.then(|| Worktree { branche: "main".into() });
                        }
                        if let Some(wt) = &mut ed.task.action.worktree {
                            ui.add(TextEdit::singleline(&mut wt.branche).desired_width(140.0));
                        }
                    });
                    ui.end_row();
                });
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Garde-fous et sortie").strong());
            ui.horizontal(|ui| {
                ui.label("Durée maximale");
                ui.add(egui::DragValue::new(&mut ed.task.garde_fous.duree_max_min).range(1..=1440).suffix(" min"));
                ui.separator();
                ui.checkbox(&mut ed.task.sortie.notification, "notifier à la fin");
            });

            ui.add_space(8.0);
            let built = ed.built();
            let problems = built.validate();
            for pb in &problems {
                ui.label(RichText::new(format!("⚠ {pb}")).color(p.warn));
            }
            if let Some(e) = &ed.error {
                ui.label(RichText::new(e).color(p.danger));
            }
            ui.horizontal(|ui| {
                let label = if problems.is_empty() { "Enregistrer" } else { "Enregistrer quand même" };
                if ui.button(RichText::new(label).strong()).on_hover_text("Une tâche invalide est gardée mais ne part pas").clicked() {
                    save = true;
                }
                if ui.button("Fermer").clicked() {
                    close = true;
                }
                if ed.id.is_some() {
                    if ed.confirm_delete {
                        ui.label(RichText::new("Supprimer ? (le fichier est mis de côté)").color(p.danger));
                        if ui.button("Oui, supprimer").clicked() {
                            delete = true;
                        }
                        if ui.button("Non").clicked() {
                            ed.confirm_delete = false;
                        }
                    } else if ui.button("Supprimer…").clicked() {
                        ed.confirm_delete = true;
                    }
                }
            });
            egui::CollapsingHeader::new("Aperçu du fichier").show(ui, |ui| {
                let json = serde_json::to_string_pretty(&built).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(json).monospace().size(12.0)).selectable(true));
            });
        });
        let t = now();
        let mut keep = !close;
        if save {
            let built = ed.built();
            match self.engine.save_task(ed.id.as_deref(), &built, t) {
                Ok(id) => {
                    self.selected_task = Some(id.clone());
                    ed = Editor::new(Some(id), built);
                    self.toast("Tâche enregistrée.", p.ok);
                }
                Err(e) => ed.error = Some(format!("Enregistrement impossible : {e}")),
            }
        }
        if delete && let Some(id) = ed.id.clone() {
            match self.engine.delete_task(&id, t) {
                Ok(()) => {
                    keep = false;
                    self.selected_task = self.engine.loaded.first().map(|l| l.id.clone());
                    self.toast("Tâche supprimée (fichier mis de côté).", p.warn);
                }
                Err(e) => ed.error = Some(format!("Suppression impossible : {e}")),
            }
        }
        if keep {
            self.editor = Some(ed);
        } else {
            self.view = View::Inbox;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut changed = false;
        egui::Window::new("Réglages").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Thème");
                ui.horizontal(|ui| {
                    for t in ThemeChoice::ALL {
                        changed |= ui.selectable_value(&mut self.state.theme, t, t.label()).changed();
                    }
                });
                ui.end_row();
                ui.label("Conserver les résultats lus");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.state.retention_jours).range(1..=3650).suffix(" jours"))
                    .changed();
                ui.end_row();
                ui.label("Notifications");
                changed |= ui.checkbox(&mut self.state.notifications, "à la fin d'une exécution").changed();
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Raccourcis").strong());
            egui::Grid::new("keys").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                for c in Command::ALL {
                    ui.label(c.label());
                    let text = if self.rebinding == Some(c) {
                        "appuyez sur une touche (Échap : annuler)".to_string()
                    } else {
                        let t = self.shortcuts.text(c);
                        if t.is_empty() { "(aucun)".into() } else { t }
                    };
                    if ui.button(text).on_hover_text("Cliquer pour changer").clicked() {
                        self.rebinding = Some(c);
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("défaut").clicked() {
                            self.shortcuts.set(c, Some(c.default_binding()));
                            changed = true;
                        }
                        if ui.small_button("aucun").clicked() {
                            self.shortcuts.set(c, None);
                            changed = true;
                        }
                        let conflicts = self.shortcuts.conflicts(c);
                        if !conflicts.is_empty() {
                            let names: Vec<_> = conflicts.iter().map(|o| o.label()).collect();
                            ui.label(
                                RichText::new(format!("⚠ aussi : {}", names.join(", ")))
                                    .color(self.palette.warn)
                                    .small(),
                            );
                        }
                    });
                    ui.end_row();
                }
            });
        });
        self.show_settings = open;
        if changed {
            self.engine.retention_days = self.state.retention_jours;
            self.apply_theme(ctx);
            self.save_state();
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        crate::theme::apply(ctx, self.state.theme);
        self.palette = crate::theme::current(ctx);
    }

    fn problems_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_problems;
        let mut clear = false;
        egui::Window::new("Problèmes").open(&mut open).default_width(520.0).show(ctx, |ui| {
            for pb in &self.engine.problems {
                ui.label(RichText::new(format!("⚠ {pb}")).color(self.palette.warn));
            }
            if ui.button("Tout effacer").clicked() {
                clear = true;
            }
        });
        if clear {
            self.engine.problems.clear();
            open = false;
        }
        self.show_problems = open;
    }

    fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }
        enum Entry {
            Cmd(Command),
            Run(String),
            Pause(String, bool),
            Edit(String),
        }
        let mut entries: Vec<(String, Entry)> = Command::ALL
            .iter()
            .filter(|c| **c != Command::Palette)
            .map(|c| (c.label().to_string(), Entry::Cmd(*c)))
            .collect();
        for l in &self.engine.loaded {
            if let Ok(t) = &l.task {
                entries.push((format!("Lancer la tâche : {}", t.nom), Entry::Run(l.id.clone())));
                let verb = if t.active { "Mettre en pause" } else { "Réactiver" };
                entries.push((format!("{verb} : {}", t.nom), Entry::Pause(l.id.clone(), !t.active)));
                entries.push((format!("Modifier : {}", t.nom), Entry::Edit(l.id.clone())));
            }
        }
        let q = self.palette_query.to_lowercase();
        let matching: Vec<&(String, Entry)> = entries
            .iter()
            .filter(|(label, _)| q.split_whitespace().all(|w| label.to_lowercase().contains(w)))
            .collect();
        let mut chosen = None;
        egui::Window::new("Palette")
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .fixed_size([460.0, 0.0])
            .show(ctx, |ui| {
                let resp = ui.add(
                    TextEdit::singleline(&mut self.palette_query)
                        .hint_text("Commande ou tâche…")
                        .desired_width(f32::INFINITY),
                );
                resp.request_focus();
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                for (i, (label, _)) in matching.iter().enumerate().take(12) {
                    if ui.selectable_label(i == 0, label.as_str()).clicked() || (enter && i == 0) {
                        chosen = Some(i);
                    }
                }
                if matching.is_empty() {
                    ui.label(RichText::new("Aucune commande.").color(self.palette.text_dim));
                }
            });
        if let Some(i) = chosen {
            self.palette_open = false;
            let act = match &matching[i].1 {
                Entry::Cmd(c) => Act::Command(*c),
                Entry::Run(id) => Act::Run(id.clone()),
                Entry::Pause(id, on) => Act::Toggle(id.clone(), *on),
                Entry::Edit(id) => Act::Edit(id.clone()),
            };
            self.acts.push(act);
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.since.elapsed() < Duration::from_secs(6));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("patronus-toasts")).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -36.0]).show(
            ctx,
            |ui| {
                for t in &self.toasts {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.label(RichText::new(&t.text).color(t.color));
                    });
                }
            },
        );
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Exécutions terminées : un message dans la fenêtre, et une notification Windows si demandé.
    fn announce_finished(&mut self) {
        for r in self.engine.take_finished() {
            let p = self.palette.clone();
            let text = format!("{} : {}", r.tache_nom, r.statut.label());
            self.toast(text.clone(), p.status(r.statut));
            let wants = self.engine.task(&r.tache).is_none_or(|t| t.sortie.notification);
            if self.state.notifications && wants && self.capture.is_none() {
                crate::notify::notify("Patronus", &format!("{text}\n{}", r.resume.lines().next().unwrap_or("")));
            }
        }
    }

    /// Capture d'essai : après 3 s, enregistre l'image puis ferme la fenêtre.
    fn capture(&mut self, ctx: &egui::Context) {
        let Some((path, start, requested)) = &mut self.capture else { return };
        let delay = std::env::var("PATRONUS_CAPTURE_DELAI").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
        if *requested || start.elapsed() < Duration::from_secs(delay) {
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }
        *requested = true;
        let (path, ctx2) = (path.clone(), ctx.clone());
        ctx.request_screenshot(move |img| {
            let png = crate::capture::png(img.size[0] as u32, img.size[1] as u32, img.as_raw());
            if let Err(e) = std::fs::write(&path, png) {
                eprintln!("capture {path} : {e}");
            }
            ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
            ctx2.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.palette = crate::theme::current(&ctx);
        if self.engine.tick(now()) {
            self.journal = None;
        }
        self.announce_finished();
        self.handle_keys(&ctx);
        let unread = self.engine.unread_count();
        let title = if unread > 0 { format!("({unread}) Patronus") } else { "Patronus".to_string() };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tasks")
            .resizable(true)
            .default_size(330.0)
            .min_size(240.0)
            .show(ui, |ui| self.task_list(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Inbox => self.inbox(ui),
            View::Editor => self.editor_view(ui),
        });
        self.settings_window(&ctx);
        self.problems_window(&ctx);
        self.command_palette(&ctx);
        self.draw_toasts(&ctx);
        for act in std::mem::take(&mut self.acts) {
            self.apply(act);
        }
        self.capture(&ctx);
        let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        if let Some(s) = size {
            self.state.taille_fenetre = [s.x, s.y];
        }
        ctx.request_repaint_after(Duration::from_millis(if self.engine.running_count() > 0 { 250 } else { 1000 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.shutdown(now(), Duration::from_secs(3));
        self.save_state();
    }
}

fn patterns_edit(ui: &mut egui::Ui, label: &str, list: &mut Vec<String>, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        let id = ui.scope_id().with(label);
        // Le texte saisi est gardé tel quel pendant la frappe, la liste suit.
        let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| list.join(", "));
        if ui.add(TextEdit::singleline(&mut text).desired_width(300.0).hint_text(hint)).changed() {
            *list = text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        }
        ui.data_mut(|d| d.insert_temp(id, text));
    });
}

/// Pastille de couleur (statut d'une exécution).
fn dot(ui: &mut egui::Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(radius * 2.0 + 2.0, radius * 2.0 + 2.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}

fn open_path(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// « aujourd'hui 08:00 », « hier 18:30 », « demain 08:00 », sinon « 21/10 08:00 ».
pub fn fmt_when(at: chrono::NaiveDateTime) -> String {
    let today = now().date();
    let day = at.date();
    let prefix = if day == today {
        "aujourd'hui".to_string()
    } else if day == today - chrono::Duration::days(1) {
        "hier".to_string()
    } else if day == today + chrono::Duration::days(1) {
        "demain".to_string()
    } else {
        at.format("%d/%m").to_string()
    };
    format!("{prefix} {}", at.format("%H:%M"))
}

pub fn fmt_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(5), "5 s");
        assert_eq!(fmt_duration(125), "2 min 05 s");
        assert_eq!(fmt_duration(3720), "1 h 02 min");
    }

    #[test]
    fn editor_splits_tools() {
        let mut t = Task::new_script("x");
        t.action.script = None;
        t.action.claude = Some(ClaudeAction { consigne: "c".into(), ..Default::default() });
        let mut ed = Editor::new(None, t);
        ed.tools = "Read, Grep,\nBash(npm outdated) ,".into();
        assert_eq!(ed.built().action.claude.unwrap().outils, ["Read", "Grep", "Bash(npm outdated)"]);
    }
}
