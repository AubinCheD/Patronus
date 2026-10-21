//! Thèmes : une palette sombre et une claire, ou celle du système.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Systeme,
    Sombre,
    Clair,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Systeme, ThemeChoice::Sombre, ThemeChoice::Clair];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Systeme => "Comme le système",
            ThemeChoice::Sombre => "Sombre",
            ThemeChoice::Clair => "Clair",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub running: Color32,
    pub border: Color32,
    pub selection: Color32,
}

impl Palette {
    pub fn dark() -> Palette {
        Palette {
            dark: true,
            bg: Color32::from_rgb(0x1e, 0x20, 0x24),
            panel: Color32::from_rgb(0x25, 0x28, 0x2d),
            text: Color32::from_rgb(0xdc, 0xdf, 0xe4),
            text_dim: Color32::from_rgb(0x8b, 0x91, 0x9a),
            accent: Color32::from_rgb(0x9f, 0xc5, 0xff),
            ok: Color32::from_rgb(0x7b, 0xc9, 0x8a),
            warn: Color32::from_rgb(0xe5, 0xc0, 0x7b),
            danger: Color32::from_rgb(0xef, 0x7a, 0x7a),
            running: Color32::from_rgb(0x8a, 0xb4, 0xf8),
            border: Color32::from_rgb(0x3a, 0x3e, 0x45),
            selection: Color32::from_rgb(0x2f, 0x46, 0x6b),
        }
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            bg: Color32::from_rgb(0xf7, 0xf7, 0xf8),
            panel: Color32::from_rgb(0xee, 0xef, 0xf1),
            text: Color32::from_rgb(0x1f, 0x23, 0x28),
            text_dim: Color32::from_rgb(0x65, 0x6d, 0x76),
            accent: Color32::from_rgb(0x1f, 0x5f, 0xbf),
            ok: Color32::from_rgb(0x1a, 0x7f, 0x37),
            warn: Color32::from_rgb(0x9a, 0x67, 0x00),
            danger: Color32::from_rgb(0xc2, 0x2b, 0x2b),
            running: Color32::from_rgb(0x1f, 0x5f, 0xbf),
            border: Color32::from_rgb(0xd0, 0xd4, 0xd9),
            selection: Color32::from_rgb(0xd3, 0xe3, 0xfd),
        }
    }

    pub fn status(&self, s: patronus_core::run::Status) -> Color32 {
        use patronus_core::run::Status::*;
        match s {
            EnCours => self.running,
            Reussie => self.ok,
            Manquee | Arretee | Interrompue => self.warn,
            Echouee | DureeDepassee | BudgetAtteint | ToursAtteints | Impossible => self.danger,
        }
    }

    pub fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.panel_fill = self.panel;
        v.window_fill = self.bg;
        v.extreme_bg_color = self.bg;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.window_stroke = Stroke::new(1.0, self.border);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        v.window_corner_radius = CornerRadius::same(6);
        v
    }
}

/// Applique les deux thèmes ; egui choisit selon la préférence (et suit le système au besoin).
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_visuals_of(egui::Theme::Light, Palette::light().visuals());
    ctx.set_visuals_of(egui::Theme::Dark, Palette::dark().visuals());
    ctx.set_theme(match choice {
        ThemeChoice::Systeme => egui::ThemePreference::System,
        ThemeChoice::Sombre => egui::ThemePreference::Dark,
        ThemeChoice::Clair => egui::ThemePreference::Light,
    });
}

/// La palette du thème affiché.
pub fn current(ctx: &egui::Context) -> Palette {
    if ctx.theme() == egui::Theme::Dark { Palette::dark() } else { Palette::light() }
}
