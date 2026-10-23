//! Patronus embarqué par un hôte (Eyrie) : ses réglages dans la page de l'hôte.

mod common;

use common::*;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use patronus_app::app::{App, Options};
use patronus_app::state::State;

/// Un hôte minimal : il dessine la page Patronus de ses réglages tant qu'elle est montrée.
struct PageHost {
    app: App,
    shown: bool,
}

impl eframe::App for PageHost {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.shown {
            self.app.settings_ui(ui);
        }
    }
}

#[test]
fn leaving_the_host_page_abandons_a_shortcut_capture() {
    const WAITING: &str = "appuyez sur une touche (Échap : annuler)";
    let dir = temp("hote-saisie");
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 800.0)).build_eframe(move |cc| {
        let mut app =
            App::with(cc, Options { data_dir: dir, state: State::default(), problems: Vec::new(), quiet: true });
        app.set_hosted(true);
        PageHost { app, shown: true }
    });
    h.run_steps(3);
    // « Lancer la tâche sélectionnée » : F5 par défaut ; cliquer lance la saisie.
    h.get_by_label("F5").click();
    h.run_steps(3);
    assert!(h.query_by_label(WAITING).is_some(), "saisie lancée");
    h.state_mut().shown = false;
    h.state_mut().app.settings_left();
    h.run_steps(2);
    h.state_mut().shown = true;
    h.run_steps(3);
    assert!(h.query_by_label(WAITING).is_none(), "page quittée : saisie abandonnée");
}
