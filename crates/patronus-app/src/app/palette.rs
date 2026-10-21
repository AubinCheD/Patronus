//! Palette de commandes : les commandes de l'application et, pour chaque tâche, lancer, mettre en
//! pause ou réactiver, modifier.

use egui::{Key, RichText, TextEdit};
use patronus_core::engine::Engine;

use super::{Act, App};
use crate::shortcuts::Command;

enum Entry {
    Cmd(Command),
    Run(String),
    Pause(String, bool),
    Edit(String),
    PollBranches,
}

impl App {
    pub(super) fn command_palette(&mut self, ctx: &egui::Context, e: &Engine) {
        if !self.palette_open {
            return;
        }
        let mut entries: Vec<(String, Entry)> = Command::ALL
            .iter()
            .filter(|c| **c != Command::Palette)
            .map(|c| (c.label().to_string(), Entry::Cmd(*c)))
            .collect();
        for l in &e.loaded {
            if let Ok(t) = &l.task {
                entries.push((format!("Lancer la tâche : {}", t.nom), Entry::Run(l.id.clone())));
                let verb = if t.active { "Mettre en pause" } else { "Réactiver" };
                entries.push((format!("{verb} : {}", t.nom), Entry::Pause(l.id.clone(), !t.active)));
                entries.push((format!("Modifier : {}", t.nom), Entry::Edit(l.id.clone())));
            }
        }
        let branches = e
            .loaded
            .iter()
            .filter_map(|l| l.task.as_ref().ok())
            .any(|t| t.declencheurs.iter().any(|tr| matches!(tr, patronus_core::task::Trigger::Branche { .. })));
        if branches {
            entries.push(("Relever les branches distantes maintenant".into(), Entry::PollBranches));
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
                Entry::PollBranches => Act::PollBranches,
            };
            self.acts.push(act);
        }
    }
}
