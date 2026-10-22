//! Symboles de l'interface, tous ici : un test (`tests/ui_symbols.rs`) vérifie que les polices de
//! l'application ont un glyphe pour chacun. Un glyphe absent s'affiche en carré vide.

pub const RUN: &str = "▶";
pub const STOP: &str = "■";
pub const EDIT: &str = "✏";
pub const REMOVE: &str = "✖";
pub const WARN: &str = "⚠";
pub const INFO: &str = "ℹ";
pub const APPROX: &str = "≈";
pub const MINUS: &str = "−";

/// Tous les symboles, pour le test des glyphes.
pub const ALL: [&str; 8] = [RUN, STOP, EDIT, REMOVE, WARN, INFO, APPROX, MINUS];

/// Signes de ponctuation du texte de l'interface (police proportionnelle).
pub const TEXT_SIGNS: &str = "«»…·—’";

/// Signes du journal (police à chasse fixe) : outils appelés par une session Claude, coupures.
pub const JOURNAL_SIGNS: &str = "→…";
