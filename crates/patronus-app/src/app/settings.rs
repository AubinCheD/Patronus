//! Fenêtres des réglages (thème, rétention, notifications, raccourcis) et des problèmes.

use egui::RichText;
use patronus_core::engine::Engine;

use super::App;
use crate::shortcuts::Command;
use crate::theme::ThemeChoice;

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context, e: &mut Engine) {
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
            e.set_retention(self.state.retention_jours);
            self.notify_on.store(self.state.notifications, std::sync::atomic::Ordering::Relaxed);
            self.apply_theme(ctx);
            self.save_state(e);
        }
    }

    pub(super) fn apply_theme(&mut self, ctx: &egui::Context) {
        crate::theme::apply(ctx, self.state.theme);
        self.palette = crate::theme::current(ctx);
    }

    pub(super) fn problems_window(&mut self, ctx: &egui::Context, e: &mut Engine) {
        let mut open = self.show_problems;
        let mut clear = false;
        egui::Window::new("Problèmes").open(&mut open).default_width(520.0).show(ctx, |ui| {
            for pb in &e.problems {
                ui.label(RichText::new(format!("⚠ {pb}")).color(self.palette.warn));
            }
            if ui.button("Tout effacer").clicked() {
                clear = true;
            }
        });
        if clear {
            e.problems.clear();
            open = false;
        }
        self.show_problems = open;
    }
}
