//! Éditeur de tâche : nom, dépôt, déclencheurs, action, garde-fous ; validation et aperçu du fichier.

use std::path::PathBuf;

use egui::{RichText, ScrollArea, TextEdit};
use patronus_core::engine::Engine;
use patronus_core::run::now;
use patronus_core::task::{ClaudeAction, FileWatch, Jour, PERMISSION_MODES, Script, Shell, Task, Trigger, Worktree};

use super::widgets::patterns_edit;
use super::{Act, App, View};

/// Une tâche en cours d'édition. `tools` est le texte saisi pour la liste des outils.
pub(super) struct Editor {
    pub id: Option<String>,
    pub task: Task,
    /// La tâche telle qu'elle était à l'ouverture de l'éditeur (ou au dernier enregistrement).
    pub original: Task,
    pub tools: String,
    pub error: Option<String>,
    pub confirm_delete: bool,
}

impl Editor {
    pub fn new(id: Option<String>, task: Task) -> Editor {
        let tools = task.action.claude.as_ref().map(|c| c.outils.join(", ")).unwrap_or_default();
        Editor { id, original: task.clone(), task, tools, error: None, confirm_delete: false }
    }

    /// Des modifications ne sont pas enregistrées.
    pub fn dirty(&self) -> bool {
        self.built() != self.original
    }

    /// La tâche telle qu'elle sera enregistrée.
    pub fn built(&self) -> Task {
        let mut t = self.task.clone();
        if let Some(c) = &mut t.action.claude {
            c.outils =
                self.tools.split([',', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        }
        t
    }
}

impl App {
    pub(super) fn editor_view(&mut self, ui: &mut egui::Ui, e: &mut Engine) {
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
                                ui.checkbox(rattrapage, "rattraper si manquée").on_hover_text(
                                    "Si Patronus était fermé à l'heure dite, lancer à l'ouverture (sinon : seulement signalé)",
                                );
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
                                        if ui
                                            .add(
                                                TextEdit::singleline(&mut d)
                                                    .desired_width(300.0)
                                                    .hint_text("C:\\dev\\northwind"),
                                            )
                                            .changed()
                                        {
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
                    ed.task
                        .declencheurs
                        .push(Trigger::Fichiers { fichiers: FileWatch { dossier, inclure: vec![], exclure: vec![] } });
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
                    ed.task.action.claude =
                        Some(ClaudeAction { budget_usd: Some(1.0), tours_max: Some(30), ..Default::default() });
                    // Une session travaille d'office dans un worktree dédié (exigé dès qu'elle peut écrire).
                    ed.task.action.worktree = Some(Worktree { branche: "main".into() });
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
                    if ui
                        .add(
                            TextEdit::singleline(&mut d)
                                .desired_width(360.0)
                                .hint_text("facultatif : le dépôt, sinon le dossier personnel"),
                        )
                        .changed()
                    {
                        s.dossier = (!d.trim().is_empty()).then(|| PathBuf::from(d.trim()));
                    }
                    ui.end_row();
                });
                ui.label(
                    RichText::new(
                        "Le script reçoit PATRONUS_TACHE, PATRONUS_DECLENCHEUR et PATRONUS_FICHIERS (fichiers modifiés).",
                    )
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
                    ui.add(
                        TextEdit::singleline(&mut ed.tools)
                            .desired_width(f32::INFINITY)
                            .hint_text("vide : lecture seule (Read, Grep, Glob) ; sinon Read, Edit, Bash(npm test)…"),
                    );
                    ui.end_row();
                    ui.label("Permissions");
                    let current = c.mode_permission.clone().unwrap_or_else(|| "default".into());
                    egui::ComboBox::new("mode", "mode")
                        .selected_text(permission_label(&current))
                        .show_ui(ui, |ui| {
                            for m in PERMISSION_MODES {
                                if ui.selectable_label(current == m, permission_label(m)).clicked() {
                                    c.mode_permission = (m != "default").then(|| m.to_string());
                                }
                            }
                        });
                    ui.end_row();
                    ui.label("Tours max");
                    let mut tours = c.tours_max.unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut tours).range(0..=500)).on_hover_text("0 : sans limite").changed() {
                        c.tours_max = (tours > 0).then_some(tours);
                    }
                    ui.end_row();
                    ui.label("Budget ($, équivalent API)");
                    let mut budget = c.budget_usd.unwrap_or(0.0);
                    if ui
                        .add(egui::DragValue::new(&mut budget).range(0.0..=100.0).speed(0.05).max_decimals(2))
                        .on_hover_text("Obligatoire : la session s'arrête au-delà")
                        .changed()
                    {
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
            for w in built.warnings() {
                ui.label(RichText::new(format!("ⓘ {w}")).color(p.text_dim));
            }
            if let Some(e) = &ed.error {
                ui.label(RichText::new(e).color(p.danger));
            }
            ui.horizontal(|ui| {
                let label = if problems.is_empty() { "Enregistrer" } else { "Enregistrer quand même" };
                if ui
                    .button(RichText::new(label).strong())
                    .on_hover_text("Une tâche invalide est gardée mais ne part pas")
                    .clicked()
                {
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
        let mut keep = true;
        if close {
            // Fermer passe par l'application : elle demande avant d'abandonner des modifications.
            self.acts.push(Act::CloseEditor);
        }
        if save {
            let built = ed.built();
            match e.save_task(ed.id.as_deref(), &built, t) {
                Ok(id) => {
                    self.selected_task = Some(id.clone());
                    ed = Editor::new(Some(id), built);
                    self.toast("Tâche enregistrée.", p.ok);
                }
                Err(err) => ed.error = Some(format!("Enregistrement impossible : {err}")),
            }
        }
        if delete && let Some(id) = ed.id.clone() {
            match e.delete_task(&id, t) {
                Ok(()) => {
                    keep = false;
                    self.selected_task = e.loaded.first().map(|l| l.id.clone());
                    self.toast("Tâche supprimée (fichier mis de côté).", p.warn);
                }
                Err(err) => ed.error = Some(format!("Suppression impossible : {err}")),
            }
        }
        if keep {
            self.editor = Some(ed);
        } else {
            self.view = View::Inbox;
        }
    }
}

/// Libellé d'un mode de permission de Claude Code.
fn permission_label(mode: &str) -> String {
    match mode {
        "default" => "par défaut (seuls les outils permis)".into(),
        "acceptEdits" => "acceptEdits (modifications de fichiers acceptées)".into(),
        "plan" => "plan (exploration, aucune modification)".into(),
        "dontAsk" => "dontAsk (tout ce qui demanderait est refusé)".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
