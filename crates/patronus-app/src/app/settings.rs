//! Fenêtres des réglages (thème, rétention, notifications, raccourcis) et des problèmes.

use crate::icons;
use egui::RichText;
use patronus_core::engine::Engine;

use super::App;
use crate::shortcuts::Command;
use crate::theme::ThemeChoice;

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context, e: &mut Engine) {
        let mut open = self.show_settings;
        egui::Window::new("Réglages")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| self.settings_contents(ui, e));
        self.show_settings = open;
    }

    /// Le contenu des réglages : dans la fenêtre, ou dans la page qu'un hôte donne à Patronus (`App::settings_ui`).
    pub(super) fn settings_contents(&mut self, ui: &mut egui::Ui, e: &mut Engine) {
        let ctx = &ui.ctx().clone();
        let hosted = self.hosted;
        let mut changed = false;
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            // Intégré : thème, taille de l'interface et démarrage de Windows sont ceux de l'hôte.
            if !hosted {
                ui.label("Thème");
                ui.horizontal(|ui| {
                    for t in ThemeChoice::ALL {
                        changed |= ui.selectable_value(&mut self.state.theme, t, t.label()).changed();
                    }
                });
                ui.end_row();
                ui.label("Taille de l'interface");
                ui.horizontal(|ui| {
                    let z = ctx.zoom_factor();
                    if ui.small_button(icons::MINUS).on_hover_text("Ctrl+Moins, Ctrl+molette").clicked() {
                        super::set_zoom(ctx, z - 0.1);
                    }
                    ui.label(format!("{:.0} %", z * 100.0));
                    if ui.small_button("+").on_hover_text("Ctrl+Plus, Ctrl+molette").clicked() {
                        super::set_zoom(ctx, z + 0.1);
                    }
                    if ui
                        .add_enabled((z - 1.0).abs() > 0.001, egui::Button::new("100 %").small())
                        .on_hover_text("Ctrl+0")
                        .clicked()
                    {
                        super::set_zoom(ctx, 1.0);
                    }
                });
                ui.end_row();
            }
            ui.label("Conserver les résultats lus");
            changed |= ui
                .add(egui::DragValue::new(&mut self.state.retention_jours).range(1..=3650).suffix(" jours"))
                .changed();
            ui.end_row();
            ui.label("Budget mensuel");
            ui.horizontal(|ui| {
                let mut usd = e.monthly_budget().unwrap_or(0.0);
                let r = ui
                    .add(egui::DragValue::new(&mut usd).range(0.0..=10_000.0).speed(0.5).max_decimals(2).suffix(" $"))
                    .on_hover_text(
                        "Plafond des sessions Claude sur le mois, en équivalent API : une session qui risquerait de \
                         le dépasser ne part pas. 0 : aucun.",
                    );
                if r.changed() {
                    e.set_monthly_budget(Some(usd));
                }
                if usd == 0.0 {
                    ui.label(RichText::new("aucun").small().color(self.palette.text_dim));
                }
            });
            ui.end_row();
            if !hosted {
                ui.label("Démarrage");
                self.autostart_row(ui, e);
                ui.end_row();
            }
            ui.label("Notifications");
            changed |= ui.checkbox(&mut self.state.notifications, "à la fin d'une exécution").changed();
            ui.end_row();
            ui.label("Données");
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.data_dir.display().to_string()).small())
                    .on_hover_text("Tâches, exécutions, réglages");
                if ui.small_button("Ouvrir").clicked() {
                    super::widgets::open_path(&self.data_dir);
                }
            });
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
                            RichText::new(format!("{} aussi : {}", icons::WARN, names.join(", ")))
                                .color(self.palette.warn)
                                .small(),
                        );
                    }
                });
                ui.end_row();
            }
        });
        if changed {
            e.set_retention(self.state.retention_jours);
            self.notify_on.store(self.state.notifications, std::sync::atomic::Ordering::Relaxed);
            if !self.hosted {
                self.apply_theme(ctx);
            }
            self.save_state(e);
        }
    }

    /// *Lancer Patronus à l'ouverture de session* : clé `Run` du compte (rien en essai).
    fn autostart_row(&mut self, ui: &mut egui::Ui, e: &mut Engine) {
        use patronus_core::autostart::{self, Status};
        let quiet = self.quiet;
        let data_dir = self.data_dir.clone();
        let status =
            self.autostart.get_or_insert_with(|| if quiet { Status::Off } else { autostart::status(&data_dir) });
        let mut on = *status == Status::On;
        ui.vertical(|ui| {
            let r = ui
                .checkbox(&mut on, "Lancer Patronus à l'ouverture de session (fenêtre réduite)")
                .on_hover_text("Clé Run du compte Windows, sans droit administrateur");
            match &*status {
                Status::OtherCommand(cmd) => {
                    ui.label(
                        RichText::new(format!("Inscrit pour un autre exécutable : {cmd}. Cocher pour le remplacer."))
                            .small()
                            .color(self.palette.warn),
                    );
                }
                Status::DisabledByUser => {
                    ui.label(
                        RichText::new("Désactivé dans le Gestionnaire des tâches. Cocher pour le réactiver.")
                            .small()
                            .color(self.palette.warn),
                    );
                }
                Status::On | Status::Off => {}
            }
            if r.changed() {
                if quiet {
                    *status = if on { Status::On } else { Status::Off };
                } else {
                    if let Err(err) = autostart::set(&data_dir, on) {
                        e.problem(format!("Lancement à l'ouverture de session : {err}"));
                    }
                    *status = autostart::status(&data_dir);
                }
            }
        });
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
                ui.label(RichText::new(format!("{} {pb}", icons::WARN)).color(self.palette.warn));
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
