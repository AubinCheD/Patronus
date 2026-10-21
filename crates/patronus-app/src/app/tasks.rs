//! Liste des tâches, à gauche : activer, lancer, arrêter, modifier ; dernière et prochaine exécution.

use crate::icons;
use egui::{Color32, RichText, ScrollArea};
use patronus_core::engine::Engine;
use patronus_core::schedule::describe;

use super::widgets::{dot, fmt_when};
use super::{Act, App};

impl App {
    pub(super) fn task_list(&mut self, ui: &mut egui::Ui, e: &Engine) {
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
        if e.loaded.is_empty() {
            ui.label(RichText::new("Aucune tâche pour l'instant.").color(p.text_dim));
            if ui.button("Créer une tâche").clicked() {
                self.acts.push(Act::NewTask);
            }
            return;
        }
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for l in &e.loaded {
                let id = &l.id;
                let selected = self.selected_task.as_deref() == Some(id.as_str());
                let frame = egui::Frame::new().inner_margin(6.0).corner_radius(4.0).fill(if selected {
                    p.selection
                } else {
                    Color32::TRANSPARENT
                });
                // Ligne cliquable enregistrée sous ses enfants : les boutons gardent leurs clics.
                let resp = ui
                    .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                        // Texte non sélectionnable : un clic sur le nom sélectionne la ligne.
                        ui.style_mut().interaction.selectable_labels = false;
                        frame.show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            match &l.task {
                                Err(why) => {
                                    ui.label(RichText::new(id).strong());
                                    ui.label(RichText::new(format!("illisible : {why}")).color(p.danger).small());
                                }
                                Ok(task) => {
                                    ui.horizontal(|ui| {
                                        let mut active = task.active;
                                        if ui
                                            .checkbox(&mut active, "")
                                            .on_hover_text("Active (sinon en pause)")
                                            .changed()
                                        {
                                            self.acts.push(Act::Toggle(id.clone(), active));
                                        }
                                        let name = RichText::new(&task.nom).strong();
                                        ui.label(if task.active { name } else { name.color(p.text_dim) });
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if e.is_running(id) {
                                                if ui.small_button(icons::STOP).on_hover_text("Arrêter").clicked() {
                                                    self.acts.push(Act::Stop(id.clone()));
                                                }
                                                ui.spinner();
                                            } else if ui
                                                .small_button(icons::RUN)
                                                .on_hover_text("Lancer maintenant")
                                                .clicked()
                                            {
                                                self.acts.push(Act::Run(id.clone()));
                                            }
                                            if ui.small_button(icons::EDIT).on_hover_text("Modifier").clicked() {
                                                self.acts.push(Act::Edit(id.clone()));
                                            }
                                        });
                                    });
                                    let problems = &l.problems;
                                    if !problems.is_empty() {
                                        ui.label(
                                            RichText::new(format!("{} {}", icons::WARN, problems[0]))
                                                .color(p.warn)
                                                .small(),
                                        );
                                    }
                                    let triggers: Vec<String> = task.declencheurs.iter().map(describe).collect();
                                    let triggers = if triggers.is_empty() {
                                        "à la main".to_string()
                                    } else {
                                        triggers.join(" ; ")
                                    };
                                    // Sur une ligne (un dossier surveillé peut avoir un long chemin), en entier au survol.
                                    ui.add(
                                        egui::Label::new(RichText::new(&triggers).small().color(p.text_dim)).truncate(),
                                    )
                                    .on_hover_text(&triggers);
                                    ui.horizontal(|ui| {
                                        if let Some(r) = e.last_run(id) {
                                            dot(ui, p.status(r.statut), 3.5);
                                            ui.label(
                                                RichText::new(format!("{} {}", r.statut.label(), fmt_when(r.debut)))
                                                    .small()
                                                    .color(p.text_dim),
                                            );
                                        }
                                        if let Some(at) = e.next_run(id) {
                                            ui.label(
                                                RichText::new(format!("· prochaine {}", fmt_when(at)))
                                                    .small()
                                                    .color(p.text_dim),
                                            );
                                        }
                                        if e.is_pending(id) {
                                            ui.label(RichText::new("· relance en attente").small().color(p.warn));
                                        }
                                    });
                                }
                            }
                        });
                    })
                    .response;
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
}
