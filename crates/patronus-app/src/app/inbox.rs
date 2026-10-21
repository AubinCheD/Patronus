//! Boîte de résultats : la liste des exécutions à gauche, le détail de celle choisie à droite.

use std::time::{Duration, Instant};

use egui::{Color32, RichText, ScrollArea};
use patronus_core::engine::Engine;
use patronus_core::run::{Run, Status};

use super::widgets::{dot, fmt_duration, fmt_when, open_path};
use super::{Act, App};

impl App {
    /// Les exécutions affichées, d'après les filtres.
    pub(super) fn visible_runs<'e>(&self, e: &'e Engine) -> Vec<&'e Run> {
        e.runs
            .iter()
            .filter(|r| self.filter_task.as_ref().is_none_or(|f| &r.tache == f))
            .filter(|r| !self.unread_only || !r.lu)
            .filter(|r| self.show_archived || !r.archive)
            .collect()
    }

    pub(super) fn inbox(&mut self, ui: &mut egui::Ui, e: &mut Engine) {
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
                    .map(|f| e.task(f).map(|t| t.nom.clone()).unwrap_or_else(|| f.clone()))
                    .unwrap_or_else(|| "Toutes les tâches".into());
                egui::ComboBox::new("filter", "tâche").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_task, None, "Toutes les tâches");
                    for l in &e.loaded {
                        let name = l.task.as_ref().map(|t| t.nom.clone()).unwrap_or_else(|_| l.id.clone());
                        ui.selectable_value(&mut self.filter_task, Some(l.id.clone()), name);
                    }
                });
                ui.checkbox(&mut self.unread_only, "Non lus");
                ui.checkbox(&mut self.show_archived, "Archivés");
            });
            ui.separator();
            let runs: Vec<Run> = self.visible_runs(e).into_iter().cloned().collect();
            if runs.is_empty() {
                ui.label(
                    RichText::new("Rien pour l'instant. Lancez une tâche (▶) ou attendez son échéance.")
                        .color(p.text_dim),
                );
            }
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for r in &runs {
                    let selected = self.selected_run.as_deref() == Some(r.id.as_str());
                    let frame = egui::Frame::new().inner_margin(6.0).corner_radius(4.0).fill(if selected {
                        p.selection
                    } else {
                        Color32::TRANSPARENT
                    });
                    let resp = ui
                        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                            ui.style_mut().interaction.selectable_labels = false;
                            frame.show(ui, |ui| {
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
                            });
                        })
                        .response;
                    if resp.clicked() {
                        self.acts.push(Act::SelectRun(r.id.clone()));
                    }
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let run = self.selected_run.as_ref().and_then(|id| e.runs.iter().find(|r| &r.id == id)).cloned();
            match run {
                None => {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Choisissez un résultat à gauche.").color(p.text_dim))
                    });
                }
                Some(r) => self.run_detail(ui, e, &r),
            }
        });
    }

    fn run_detail(&mut self, ui: &mut egui::Ui, e: &mut Engine, r: &Run) {
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
            let (approx, note) = if c.estime { ("≈ ", " (estimé par Patronus)") } else { ("", "") };
            facts.push(format!(
                "{approx}{} + {} tokens · {approx}{:.2} $ équivalent API{note}",
                c.tokens_entree, c.tokens_sortie, c.usd
            ));
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
                    e.mark_read(&r.id, false);
                }
                if !r.archive && ui.button("Archiver").clicked() {
                    e.archive(&r.id);
                }
            }
            if ui.button("Ouvrir le dossier").on_hover_text("Journal et fichiers de l'exécution").clicked() {
                open_path(&e.store.run_dir(&r.id));
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
            self.journal = Some((r.id.clone(), e.store.read_journal(&r.id), Instant::now()));
        }
        let diff = e.store.read_diff(&r.id);
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
}
