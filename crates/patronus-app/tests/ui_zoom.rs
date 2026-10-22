//! Zoom de l'interface : réglages, Ctrl+molette, Ctrl+0 ; retenu d'une ouverture à l'autre.

mod common;

use common::*;
use eframe::egui;
use patronus_app::state;

fn ctrl_wheel(h: &mut H, lines: f32) {
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, lines),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.step();
    h.step();
}

#[test]
fn zoom_is_set_and_kept() {
    let dir = temp("zoom");
    let mut h = app(&dir);
    assert_eq!(h.ctx.zoom_factor(), 1.0);
    click(&mut h, "Réglages");
    click_near(&mut h, "+", "Taille de l'interface");
    wait_until(&mut h, "zoom à 110 %", |h| (h.ctx.zoom_factor() - 1.1).abs() < 0.001);
    assert!(shows(&h, "110 %"));
    ctrl_wheel(&mut h, 3.0);
    let z = h.ctx.zoom_factor();
    assert!(z > 1.1, "Ctrl+molette agrandit : {z}");
    eframe::App::on_exit(h.state_mut(), None);
    let (saved, _) = state::load(&dir);
    assert!((saved.zoom - z).abs() < 0.001, "retenu : {} au lieu de {z}", saved.zoom);
    drop(h);
    let mut h = app_with(&dir, saved);
    assert!((h.ctx.zoom_factor() - z).abs() < 0.001, "rouvert au même zoom");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num0);
    h.step();
    wait_until(&mut h, "Ctrl+0 : 100 %", |h| (h.ctx.zoom_factor() - 1.0).abs() < 0.001);
}
