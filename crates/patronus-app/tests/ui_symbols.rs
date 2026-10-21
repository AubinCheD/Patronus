//! Chaque symbole de l'interface a un glyphe dans les polices de l'application (sinon egui affiche
//! un carré vide, comme au premier jet pour ▶, ■ et ✏).

mod common;

use common::*;
use eframe::egui::{FontFamily, FontId};
use patronus_app::icons;

#[test]
fn every_symbol_has_a_glyph() {
    let dir = temp("symboles");
    let h = app(&dir);
    let proportional = FontId::new(14.0, FontFamily::Proportional);
    let monospace = FontId::new(12.0, FontFamily::Monospace);
    let mut missing = Vec::new();
    h.ctx.fonts_mut(|f| {
        for c in icons::ALL.concat().chars().chain(icons::TEXT_SIGNS.chars()) {
            if !f.has_glyph(&proportional, c) {
                missing.push(format!("{c} (U+{:04X}, police proportionnelle)", c as u32));
            }
        }
        for c in icons::JOURNAL_SIGNS.chars() {
            if !f.has_glyph(&monospace, c) {
                missing.push(format!("{c} (U+{:04X}, police à chasse fixe)", c as u32));
            }
        }
    });
    assert!(missing.is_empty(), "glyphes absents : {missing:?}");
}
