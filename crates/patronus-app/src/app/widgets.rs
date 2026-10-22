//! Petits éléments partagés par les vues : pastille, champ de motifs, dates et durées.

use std::path::Path;

use egui::{Color32, TextEdit};
use patronus_core::run::now;

/// Champ texte d'une liste de motifs séparés par des virgules. Le texte saisi est gardé tel quel
/// pendant la frappe (une virgule finale ne disparaît pas), la liste suit.
pub fn patterns_edit(ui: &mut egui::Ui, label: &str, list: &mut Vec<String>, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        let id = ui.scope_id().with(label);
        let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| list.join(", "));
        if ui.add(TextEdit::singleline(&mut text).desired_width(300.0).hint_text(hint)).changed() {
            *list = text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        }
        ui.data_mut(|d| d.insert_temp(id, text));
    });
}

/// Pastille de couleur (statut d'une exécution), dessinée : aucun glyphe à trouver dans la police.
pub fn dot(ui: &mut egui::Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(radius * 2.0 + 2.0, radius * 2.0 + 2.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}

pub fn open_path(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Ouvre une adresse `http://` ou `https://` dans le navigateur par défaut ; rien d'autre (un lien
/// vient de la sortie d'un script).
pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    let null = std::ptr::null();
    unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), null, null, SW_SHOWNORMAL) };
}

/// « aujourd'hui 08:00 », « hier 18:30 », « demain 08:00 », sinon « 21/10 08:00 ».
pub fn fmt_when(at: chrono::NaiveDateTime) -> String {
    let today = now().date();
    let day = at.date();
    let prefix = if day == today {
        "aujourd'hui".to_string()
    } else if day == today - chrono::Duration::days(1) {
        "hier".to_string()
    } else if day == today + chrono::Duration::days(1) {
        "demain".to_string()
    } else {
        at.format("%d/%m").to_string()
    };
    format!("{prefix} {}", at.format("%H:%M"))
}

pub fn fmt_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(5), "5 s");
        assert_eq!(fmt_duration(125), "2 min 05 s");
        assert_eq!(fmt_duration(3720), "1 h 02 min");
    }
}
