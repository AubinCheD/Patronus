//! La fenêtre : tâches à gauche, boîte de résultats ou éditeur au centre, barre d'état en bas.
//!
//! Le moteur tourne dans son propre fil (`Ticker`) : les échéances partent même fenêtre réduite.
//! L'interface le verrouille le temps d'une image ; ce que l'utilisateur demande pendant le dessin est
//! noté (`Act`) puis appliqué à la fin de l'image, sous le même verrou.

mod editor;
mod inbox;
mod palette;
mod settings;
mod tasks;
mod widgets;

use crate::icons;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use egui::{Color32, Key, RichText};
use patronus_core::engine::{Engine, Options as EngineOptions, Shared, Ticker, lock};
use patronus_core::exec::Context;
use patronus_core::run::{Status, now};
use patronus_core::task::Task;

use self::editor::Editor;
use self::widgets::fmt_when;
use crate::shortcuts::{Binding, Command, Shortcuts};
use crate::state::{self, State};
use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Inbox,
    Editor,
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
    /// Fermer l'éditeur (après confirmation s'il y a des modifications).
    CloseEditor,
    /// Relever les branches distantes sans attendre.
    PollBranches,
    /// Actions sur un résultat : demander confirmation, puis agir.
    AskApplyDiff(String),
    ApplyDiff(String),
    AskDropWorktree(String),
    DropWorktree(String),
    ResumeSession(String),
    Command(Command),
}

/// Confirmation d'une action sur un résultat.
enum RunDialog {
    /// Appliquer le diff : ce qu'il toucherait, ou pourquoi c'est impossible.
    Apply(String, Result<patronus_core::actions::ApplyPlan, String>),
    DropWorktree(String),
}

/// Ce qu'il faut pour ouvrir la fenêtre.
pub struct Options {
    pub data_dir: PathBuf,
    pub state: State,
    /// Problèmes déjà relevés (réglages illisibles…), affichés dès l'ouverture.
    pub problems: Vec<String>,
    /// Essais : ni notification Windows, ni Planificateur, ni registre.
    pub quiet: bool,
}

pub struct App {
    engine: Shared,
    /// Le fil du moteur ; arrêté avant la fermeture du moteur.
    ticker: Option<Ticker>,
    /// Fins d'exécution annoncées par le fil du moteur, à montrer dans la fenêtre.
    finished_rx: Receiver<(String, Status)>,
    /// Notifications Windows voulues (réglage), lu par le fil du moteur.
    notify_on: Arc<AtomicBool>,
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
    /// Journal et diff de l'exécution affichée.
    shown: Option<inbox::Shown>,
    capture: Option<(String, Instant, bool)>,
    acts: Vec<Act>,
    /// Dernier titre envoyé à la fenêtre (l'envoyer la redessine).
    title: String,
    /// Action en attente de confirmation : elle abandonnerait des modifications de l'éditeur.
    confirm: Option<Act>,
    /// Confirmation d'une action sur un résultat.
    run_dialog: Option<RunDialog>,
    /// Exécution choisie pendant qu'elle tournait : marquée lue à sa fin si elle est toujours affichée.
    read_when_done: Option<String>,
    /// Synchronisation du Planificateur de Windows : faite (version des tâches), et ses problèmes.
    windows_tasks: Option<(u64, Receiver<Vec<String>>)>,
    /// Essais : ni Planificateur ni registre.
    quiet: bool,
    /// Coût des sessions du mois, relu toutes les 5 s (barre d'état).
    month_cost: Option<(Instant, f64)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, data_dir: PathBuf, state: State, problems: Vec<String>) -> App {
        App::with(cc, Options { data_dir, state, problems, quiet: false })
    }

    pub fn with(cc: &eframe::CreationContext<'_>, opts: Options) -> App {
        let Options { data_dir, state, mut problems, quiet } = opts;
        let (shortcuts, sp) = Shortcuts::from_saved(&state.raccourcis);
        problems.extend(sp);
        let opts = EngineOptions { retention_days: state.retention_jours, ..EngineOptions::default() };
        let mut engine = Engine::open_opts(&data_dir, now(), opts);
        engine.problems.splice(0..0, problems);
        crate::theme::apply(&cc.egui_ctx, state.theme);
        // Essais : `PATRONUS_ZOOM` agrandit l'interface (captures lisibles en taille réelle).
        if let Some(z) = std::env::var("PATRONUS_ZOOM").ok().and_then(|z| z.parse::<f32>().ok()) {
            cc.egui_ctx.set_zoom_factor(z.clamp(0.5, 3.0));
        }
        let palette = crate::theme::current(&cc.egui_ctx);
        let capture = std::env::var("PATRONUS_CAPTURE").ok().map(|p| (p, Instant::now(), false));
        let selected_task = engine.loaded.first().map(|l| l.id.clone());
        let selected_run = engine.runs.first().map(|r| r.id.clone());
        let engine: Shared = Arc::new(Mutex::new(engine));
        let notify_on = Arc::new(AtomicBool::new(state.notifications));
        let (finished_tx, finished_rx) = channel();
        let windows_notifications = !quiet && capture.is_none();
        let (ctx, flag) = (cc.egui_ctx.clone(), notify_on.clone());
        let ticker = Ticker::start(engine.clone(), Duration::from_millis(250), now, move |e, changed| {
            let finished = e.take_finished();
            for r in &finished {
                let text = format!("{} : {}", r.tache_nom, r.statut.label());
                let _ = finished_tx.send((text.clone(), r.statut));
                let wants = e.task(&r.tache).is_none_or(|t| t.sortie.notification);
                if windows_notifications && wants && flag.load(Ordering::Relaxed) {
                    crate::notify::notify("Patronus", &format!("{text}\n{}", r.resume.lines().next().unwrap_or("")));
                }
            }
            if changed || !finished.is_empty() {
                ctx.request_repaint();
            }
        });
        let mut app = App {
            engine,
            ticker: Some(ticker),
            finished_rx,
            notify_on,
            data_dir,
            state,
            shortcuts,
            palette,
            view: View::Inbox,
            selected_task,
            selected_run,
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
            shown: None,
            capture,
            acts: Vec::new(),
            title: String::new(),
            confirm: None,
            run_dialog: None,
            read_when_done: None,
            windows_tasks: None,
            quiet,
            month_cost: None,
        };
        // Essais : la vue à montrer dans la capture.
        match std::env::var("PATRONUS_CAPTURE_VUE").as_deref() {
            Ok("editeur") => {
                let shared = app.engine.clone();
                let e = lock(&shared);
                match app.selected_task.clone() {
                    Some(id) => app.open_editor(&id, &e),
                    None => app.new_task(),
                }
            }
            Ok("reglages") => app.show_settings = true,
            Ok("palette") => app.palette_open = true,
            _ => {}
        }
        app
    }

    /// La tâche sur laquelle la boîte est filtrée.
    pub fn inbox_filter(&self) -> Option<&str> {
        self.filter_task.as_deref()
    }

    /// La tâche en cours d'édition, telle qu'elle serait enregistrée.
    pub fn draft(&self) -> Option<Task> {
        self.editor.as_ref().map(|e| e.built())
    }

    /// Le moteur, verrouillé (tests) : ne pas garder le verrou pendant une image.
    pub fn engine(&self) -> MutexGuard<'_, Engine> {
        lock(&self.engine)
    }

    fn save_state(&mut self, e: &mut Engine) {
        self.state.raccourcis = self.shortcuts.to_saved();
        if let Err(err) = state::save(&self.data_dir, &self.state) {
            e.problems.push(format!("Enregistrement des réglages impossible : {err}"));
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, since: Instant::now() });
    }

    fn open_editor(&mut self, id: &str, e: &Engine) {
        // Déjà ouverte : on revient à son brouillon, tel quel.
        if self.editor.as_ref().is_some_and(|ed| ed.id.as_deref() == Some(id)) {
            self.view = View::Editor;
            self.selected_task = Some(id.to_string());
            return;
        }
        if let Some(t) = e.task(id) {
            self.editor = Some(Editor::new(Some(id.to_string()), t.clone()));
            self.view = View::Editor;
            self.selected_task = Some(id.to_string());
        } else if let Some(l) = e.loaded.iter().find(|l| l.id == id) {
            let why = l.task.as_ref().err().cloned().unwrap_or_default();
            self.toast(format!("{} : {why} — corriger le fichier {}", l.id, l.path.display()), self.palette.danger);
        }
    }

    fn new_task(&mut self) {
        self.editor = Some(Editor::new(None, Task::new_script("Nouvelle tâche")));
        self.view = View::Editor;
    }

    fn apply(&mut self, act: Act, e: &mut Engine) {
        let t = now();
        // Ouvrir une autre tâche, en créer une ou fermer l'éditeur abandonnerait le brouillon : on
        // demande d'abord.
        if let Some(ed) = &self.editor
            && ed.dirty()
            && match &act {
                Act::Edit(id) => ed.id.as_ref() != Some(id),
                Act::NewTask | Act::CloseEditor => true,
                _ => false,
            }
        {
            self.confirm = Some(act);
            return;
        }
        match act {
            Act::CloseEditor => {
                self.editor = None;
                self.view = View::Inbox;
            }
            Act::Run(id) => {
                let queued = e.is_running(&id);
                e.launch(&id, Context { trigger: "à la main".into(), files: vec![], ..Default::default() }, t);
                if queued {
                    self.toast("Déjà en cours : relancée à la fin.", self.palette.warn);
                }
                self.view = View::Inbox;
                if let Some(r) = e.runs.first() {
                    self.selected_run = Some(r.id.clone());
                }
            }
            Act::Stop(id) => e.stop(&id),
            Act::StopAll => {
                let n = e.running_count();
                e.stop_all();
                self.toast(format!("Arrêt demandé pour {n} exécution(s)."), self.palette.warn);
            }
            Act::Edit(id) => self.open_editor(&id, e),
            Act::NewTask => self.new_task(),
            Act::Toggle(id, active) => {
                if let Some(mut task) = e.task(&id).cloned() {
                    task.active = active;
                    if let Err(err) = e.save_task(Some(&id), &task, t) {
                        self.toast(format!("Enregistrement impossible : {err}"), self.palette.danger);
                    }
                }
            }
            Act::SelectRun(id) => {
                self.selected_run = Some(id.clone());
                // Une exécution en cours sera marquée lue à sa fin, si on la regarde encore.
                if e.runs.iter().any(|r| r.id == id && r.statut == Status::EnCours) {
                    self.read_when_done = Some(id);
                } else {
                    e.mark_read(&id, true);
                }
                self.view = View::Inbox;
            }
            Act::Inbox => self.view = View::Inbox,
            Act::AskApplyDiff(id) => {
                let plan = e.diff_plan(&id);
                self.run_dialog = Some(RunDialog::Apply(id, plan));
            }
            Act::ApplyDiff(id) => {
                self.run_dialog = None;
                match e.apply_diff(&id, t) {
                    Ok(plan) => self.toast(
                        format!("Diff appliqué : {} fichier(s), sans commit.", plan.files.len()),
                        self.palette.ok,
                    ),
                    Err(err) => self.toast(format!("Diff non appliqué : {err}"), self.palette.danger),
                }
            }
            Act::AskDropWorktree(id) => self.run_dialog = Some(RunDialog::DropWorktree(id)),
            Act::DropWorktree(id) => {
                self.run_dialog = None;
                match e.drop_worktree(&id) {
                    Ok(()) => self.toast("Worktree supprimé ; le diff reste dans le résultat.", self.palette.ok),
                    Err(err) => self.toast(format!("Worktree non supprimé : {err}"), self.palette.danger),
                }
            }
            Act::ResumeSession(id) => {
                if let Err(err) = e.resume_launch(&id).and_then(|l| l.spawn().map_err(|x| x.to_string())) {
                    self.toast(format!("Session non reprise : {err}"), self.palette.danger);
                }
            }
            Act::PollBranches => {
                e.poll_branches_now();
                self.toast("Relevé des branches distantes lancé.", self.palette.text_dim);
            }
            Act::Command(c) => self.command(c, e),
        }
    }

    fn command(&mut self, c: Command, e: &mut Engine) {
        match c {
            Command::NewTask => self.new_task(),
            Command::Inbox => self.view = View::Inbox,
            Command::RunSelected => {
                if let Some(id) = self.selected_task.clone() {
                    self.apply(Act::Run(id), e);
                }
            }
            Command::StopAll => self.apply(Act::StopAll, e),
            Command::Palette => {
                self.palette_open = !self.palette_open;
                self.palette_query.clear();
            }
            Command::Settings => self.show_settings = !self.show_settings,
            Command::NextUnread => {
                let next = self
                    .visible_runs(e)
                    .into_iter()
                    .find(|r| !r.lu && r.statut != Status::EnCours)
                    .map(|r| r.id.clone());
                match next {
                    Some(id) => self.apply(Act::SelectRun(id), e),
                    None => self.toast("Aucun résultat non lu.", self.palette.text_dim),
                }
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context, e: &mut Engine) {
        if let Some(cmd) = self.rebinding {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|ev| match ev {
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
                    self.save_state(e);
                }
            }
            return;
        }
        let pressed: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|ev| match ev {
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
            // Pendant une saisie, une touche sans Ctrl ni Alt (F5 compris) ne déclenche rien.
            let typing = ctx.memory(|m| m.focused().is_some());
            let plain = !mods.ctrl && !mods.command && !mods.alt;
            if typing && plain {
                continue;
            }
            if let Some(c) = self.shortcuts.command_for(key, mods) {
                self.command(c, e);
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, e: &Engine) {
        let p = self.palette.clone();
        // Les boutons passent à la ligne quand la fenêtre est étroite (ou l'interface agrandie).
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Patronus").strong().color(p.accent));
            ui.separator();
            let unread = e.unread_count();
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
            // Un brouillon ouvert reste à portée quand on regarde la boîte.
            if self.view != View::Editor
                && let Some(ed) = &self.editor
            {
                let nom = if ed.task.nom.trim().is_empty() { "nouvelle tâche" } else { ed.task.nom.trim() };
                let label =
                    format!("{} Reprendre « {nom} »{}", icons::EDIT, if ed.dirty() { " (modifiée)" } else { "" });
                if ui.button(label).clicked() {
                    self.view = View::Editor;
                }
            }
            let running = e.running_count();
            if ui
                .add_enabled(running > 0, egui::Button::new(format!("{} Tout arrêter", icons::STOP)))
                .on_hover_text(self.shortcuts.text(Command::StopAll))
                .clicked()
            {
                self.acts.push(Act::StopAll);
            }
            ui.separator();
            if ui.button("Commandes…").on_hover_text(self.shortcuts.text(Command::Palette)).clicked() {
                self.palette_open = true;
            }
            if ui.button("Réglages").on_hover_text(self.shortcuts.text(Command::Settings)).clicked() {
                self.show_settings = !self.show_settings;
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, e: &Engine) {
        let p = self.palette.clone();
        ui.horizontal(|ui| {
            let running = e.running_count();
            if running > 0 {
                ui.spinner();
                ui.label(RichText::new(format!("{running} en cours")).color(p.running));
                ui.separator();
            }
            let next = e
                .loaded
                .iter()
                .filter_map(|l| {
                    e.next_run(&l.id).map(|at| (at, l.task.as_ref().map(|t| t.nom.clone()).unwrap_or_default()))
                })
                .min_by_key(|(at, _)| *at);
            match next {
                Some((at, nom)) => {
                    ui.label(RichText::new(format!("Prochaine : {nom}, {}", fmt_when(at))).color(p.text_dim))
                }
                None => ui.label(RichText::new("Aucune échéance planifiée").color(p.text_dim)),
            };
            if self.month_cost.is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(5)) {
                self.month_cost = Some((Instant::now(), e.month_cost(now())));
            }
            let spent = self.month_cost.map_or(0.0, |(_, usd)| usd);
            let budget = e.monthly_budget();
            if spent > 0.0 || budget.is_some() {
                ui.separator();
                let (text, color) = match budget {
                    Some(b) => (
                        format!("Ce mois : {spent:.2} $ sur {b:.2} $"),
                        if spent >= b * 0.8 { p.warn } else { p.text_dim },
                    ),
                    None => (format!("Ce mois : {spent:.2} $"), p.text_dim),
                };
                ui.label(RichText::new(text).color(color))
                    .on_hover_text("Sessions Claude, en équivalent API (tarif public, pas une facture)");
            }
            let n = e.problems.len();
            if n > 0 {
                ui.separator();
                if ui.link(RichText::new(format!("{} {n} problème(s)", icons::WARN)).color(p.warn)).clicked() {
                    self.show_problems = true;
                }
            }
        });
    }

    /// Met le Planificateur de Windows en accord avec les tâches (`hors_application`), dans un fil
    /// (`schtasks` prend du temps) : à l'ouverture et après chaque relecture des tâches.
    fn sync_windows_tasks(&mut self, e: &mut Engine) {
        if self.quiet {
            return;
        }
        if let Some((_, rx)) = &self.windows_tasks
            && let Ok(problems) = rx.try_recv()
        {
            for p in problems {
                e.problem(p);
            }
        }
        if self.windows_tasks.as_ref().is_some_and(|(v, _)| *v == e.tasks_version) {
            return;
        }
        let tasks: Vec<(String, Task)> =
            e.loaded.iter().filter_map(|l| l.task.as_ref().ok().map(|t| (l.id.clone(), t.clone()))).collect();
        let data_dir = self.data_dir.clone();
        let (tx, rx) = channel();
        self.windows_tasks = Some((e.tasks_version, rx));
        std::thread::spawn(move || {
            use patronus_core::winsched;
            let folder = winsched::folder(&data_dir, &patronus_core::fsutil::default_data_dir());
            let wanted = winsched::wanted_set(&tasks, &winsched::current_exe(), &data_dir, now().date());
            let _ = tx.send(winsched::sync(&folder, &wanted, &data_dir.join("planificateur")));
        });
    }

    /// Confirmation d'une action sur un résultat (appliquer le diff, supprimer le worktree).
    fn run_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.run_dialog else { return };
        let (mut go, mut cancel) = (None, false);
        egui::Modal::new(egui::Id::unique("action-resultat")).show(ctx, |ui| {
            ui.set_width(460.0);
            match dialog {
                RunDialog::Apply(id, Ok(plan)) => {
                    ui.heading("Appliquer le diff");
                    ui.label(format!(
                        "Dans la copie de travail (branche {}), sans commit ni index : {} fichier(s).",
                        if plan.branch.is_empty() { "?" } else { &plan.branch },
                        plan.files.len()
                    ));
                    for f in plan.files.iter().take(12) {
                        ui.label(RichText::new(f).monospace().small());
                    }
                    if plan.files.len() > 12 {
                        ui.label(format!("… et {} autres", plan.files.len() - 12));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Appliquer").clicked() {
                            go = Some(Act::ApplyDiff(id.clone()));
                        }
                        cancel = ui.button("Annuler").clicked();
                    });
                }
                RunDialog::Apply(_, Err(why)) => {
                    ui.heading("Diff non applicable");
                    ui.label(why.as_str());
                    cancel = ui.button("Fermer").clicked();
                }
                RunDialog::DropWorktree(id) => {
                    ui.heading("Supprimer le worktree");
                    ui.label("Le dossier du worktree est supprimé ; le diff reste dans le résultat.");
                    ui.horizontal(|ui| {
                        if ui.button("Supprimer").clicked() {
                            go = Some(Act::DropWorktree(id.clone()));
                        }
                        cancel = ui.button("Annuler").clicked();
                    });
                }
            }
        });
        if let Some(act) = go {
            self.acts.push(act);
        } else if cancel {
            self.run_dialog = None;
        }
    }

    /// Confirmation avant d'abandonner les modifications de l'éditeur.
    fn confirm_window(&mut self, ctx: &egui::Context, e: &mut Engine) {
        if self.confirm.is_none() {
            return;
        }
        let nom = self.editor.as_ref().map(|ed| ed.task.nom.clone()).unwrap_or_default();
        let (mut abandon, mut back) = (false, false);
        egui::Modal::new(egui::Id::unique("abandon-brouillon")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Modifications non enregistrées");
            ui.label(format!("Les modifications de « {nom} » ne sont pas enregistrées."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                abandon = ui.button("Abandonner les modifications").clicked();
                back = ui.button("Revenir au brouillon").clicked();
            });
        });
        if abandon && let Some(act) = self.confirm.take() {
            self.editor = None;
            self.apply(act, e);
        } else if back {
            self.confirm = None;
            self.view = View::Editor;
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        while let Ok((text, status)) = self.finished_rx.try_recv() {
            let color = self.palette.status(status);
            self.toast(text, color);
            self.shown = None;
        }
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

    /// Capture d'essai : après quelques secondes, enregistre l'image puis ferme la fenêtre.
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
        let shared = self.engine.clone();
        let mut e = lock(&shared);
        self.handle_keys(&ctx, &mut e);
        let unread = e.unread_count();
        let title = if unread > 0 { format!("({unread}) Patronus") } else { "Patronus".to_string() };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui, &e);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, &e));
        egui::Panel::left("tasks")
            .resizable(true)
            .default_size(330.0)
            .min_size(240.0)
            .show(ui, |ui| self.task_list(ui, &e));
        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Inbox => self.inbox(ui, &mut e),
            View::Editor => self.editor_view(ui, &mut e),
        });
        self.settings_window(&ctx, &mut e);
        self.problems_window(&ctx, &mut e);
        self.command_palette(&ctx, &e);
        self.confirm_window(&ctx, &mut e);
        self.run_dialog_window(&ctx);
        // Exécution choisie pendant qu'elle tournait : lue à sa fin si elle est toujours affichée.
        if let Some(id) = self.read_when_done.clone()
            && let Some(r) = e.runs.iter().find(|r| r.id == id)
            && r.statut != Status::EnCours
        {
            if self.view == View::Inbox && self.selected_run.as_deref() == Some(id.as_str()) {
                e.mark_read(&id, true);
            }
            self.read_when_done = None;
        }
        self.draw_toasts(&ctx);
        for act in std::mem::take(&mut self.acts) {
            self.apply(act, &mut e);
        }
        self.sync_windows_tasks(&mut e);
        let running = e.running_count();
        drop(e);
        self.capture(&ctx);
        let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        if let Some(s) = size {
            // En points de l'écran, hors zoom de l'interface : la fenêtre garde sa taille quel que soit le zoom.
            let zoom = ctx.zoom_factor();
            self.state.taille_fenetre = [s.x * zoom, s.y * zoom];
        }
        ctx.request_repaint_after(Duration::from_millis(if running > 0 { 250 } else { 1000 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // D'abord le fil (il ne relancera plus rien), puis l'arrêt propre des exécutions.
        self.ticker = None;
        let shared = self.engine.clone();
        let mut e = lock(&shared);
        e.shutdown(now(), Duration::from_secs(3));
        self.save_state(&mut e);
    }
}
