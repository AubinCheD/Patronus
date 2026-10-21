//! Interface de Patronus (eframe/egui, rendu glow). L'exécutable n'est qu'une enveloppe : la suite
//! pourra charger la même interface comme module.

pub mod app;
pub mod capture;
pub mod notify;
pub mod shortcuts;
pub mod state;
pub mod theme;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub const APP_TITLE: &str = "Patronus";

/// Ouvre la fenêtre sur le dossier de données `data_dir`.
pub fn run(data_dir: std::path::PathBuf) -> eframe::Result {
    let (st, problems) = state::load(&data_dir);
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_TITLE)
        .with_app_id("patronus")
        .with_inner_size(st.taille_fenetre)
        .with_min_inner_size([760.0, 480.0]);
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |cc| {
            if let Ok(h) = cc.window_handle()
                && let RawWindowHandle::Win32(w) = h.as_raw()
            {
                notify::init(w.hwnd.get());
            }
            Ok(Box::new(app::App::new(cc, data_dir, st, problems)))
        }),
    )
}
