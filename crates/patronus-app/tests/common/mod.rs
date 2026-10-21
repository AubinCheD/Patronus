//! Outils partagés des tests d'interface (egui_kittest) : l'application entière dans un dossier de
//! données temporaire, attentes, clics, saisie. Mêmes procédés que Menagerie et DragonTree.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use patronus_app::app::{App, Options};
use patronus_app::state::State;
use patronus_core::task::{Script, Shell, Task, TaskStore};

pub type H = Harness<'static, App>;

/// Dossier temporaire propre au test, vidé.
pub fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("patronus-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// L'application sur le dossier de données `dir`.
pub fn app(dir: &Path) -> H {
    app_with(dir, State::default())
}

pub fn app_with(dir: &Path, state: State) -> H {
    let dir = dir.to_path_buf();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1300.0, 850.0))
        .build_eframe(move |cc| App::with(cc, Options { data_dir: dir, state, problems: Vec::new(), quiet: true }));
    h.step();
    h
}

/// Une tâche script cmd enregistrée avant l'ouverture de l'application.
pub fn script_task(dir: &Path, id: &str, nom: &str, commande: &str) -> Task {
    let mut t = Task::new_script(nom);
    t.action.script = Some(Script {
        commande: commande.into(),
        shell: Shell::Cmd,
        dossier: Some(dir.to_path_buf()),
        ..Default::default()
    });
    TaskStore::new(dir.join("taches")).save(id, &t).unwrap();
    t
}

/// Fait tourner l'interface jusqu'à ce que `cond` soit vraie.
pub fn wait_until(h: &mut H, what: &str, mut cond: impl FnMut(&H) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        h.step();
        if cond(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("délai dépassé en attendant : {what}");
}

/// Texte d'un élément tel que le cherchent les `*_by_label` de kittest : celui d'un `Label` est sa valeur.
pub fn text_of(node: &AccessKitNode<'_>) -> Option<String> {
    if node.role() == egui::accesskit::Role::Label { node.value() } else { node.label() }
}

/// Un élément de libellé exactement `label` est affiché.
pub fn shows(h: &H, label: &str) -> bool {
    h.query_all_by(|n| text_of(n).as_deref() == Some(label)).next().is_some()
}

/// Un élément dont le libellé contient `part` est affiché.
pub fn shows_containing(h: &H, part: &str) -> bool {
    h.query_all_by(|n| text_of(n).is_some_and(|t| t.contains(part))).next().is_some()
}

/// Clique `pos` en une seule image : survol, appui et relâchement arrivent ensemble (`Node::click`
/// les répartit sur trois images, et le clic tombe à côté si l'interface bouge entre-temps).
pub fn click_at(h: &mut H, pos: egui::Pos2) {
    let events = &mut h.input_mut().events;
    events.push(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.step();
    h.step();
}

/// Attend l'élément de libellé exactement `label`, unique et utilisable, puis le clique.
pub fn click(h: &mut H, label: &str) {
    let find = |h: &H| {
        let all: Vec<_> = h.query_all_by(|n| text_of(n).as_deref() == Some(label)).collect();
        (all.len() == 1 && !all[0].accesskit_node().is_disabled()).then(|| all[0].rect().center())
    };
    wait_until(h, &format!("« {label} » affiché, unique et utilisable"), |h| find(h).is_some());
    let pos = find(h).unwrap();
    click_at(h, pos);
}

/// Clique l'élément d'infobulle ou de libellé `label` le plus proche (verticalement) du texte `near`.
pub fn click_near(h: &mut H, label: &str, near: &str) {
    let find = |h: &H| {
        let anchor = h.query_all_by(|n| text_of(n).as_deref() == Some(near)).next()?.rect().center();
        h.query_all_by(|n| text_of(n).as_deref() == Some(label))
            .map(|n| n.rect().center())
            .min_by(|a, b| (a.y - anchor.y).abs().total_cmp(&(b.y - anchor.y).abs()))
    };
    wait_until(h, &format!("« {label} » près de « {near} »"), |h| find(h).is_some());
    let pos = find(h).unwrap();
    click_at(h, pos);
}

/// Saisit `text` dans le champ (vide) de texte d'aide `placeholder`.
pub fn type_in(h: &mut H, placeholder: &str, text: &str) {
    wait_until(h, &format!("champ « {placeholder} »"), |h| {
        h.query_by(|n| n.placeholder() == Some(placeholder)).is_some()
    });
    let f = h.get_by(|n| n.placeholder() == Some(placeholder));
    f.focus();
    f.type_text(text);
    h.step();
    h.step();
}

/// Demande la fermeture de la fenêtre, comme le bouton de la barre de titre.
pub fn request_close(h: &mut H) {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();
    h.step();
}

/// Remplace le texte du champ qui contient exactement `old` par `new`.
pub fn replace_text(h: &mut H, old: &str, new: &str) {
    let is_field = |n: &AccessKitNode<'_>, v: &str| {
        matches!(n.role(), egui::accesskit::Role::TextInput | egui::accesskit::Role::MultilineTextInput)
            && n.value().as_deref() == Some(v)
    };
    wait_until(h, &format!("champ « {old} »"), |h| h.query_by(|n| is_field(n, old)).is_some());
    h.get_by(|n| is_field(n, old)).focus();
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    h.get_by(|n| is_field(n, old)).type_text(new);
    h.step();
    h.step();
}

/// L'application, posée : plus aucun rafraîchissement demandé en continu.
pub fn settles(h: &mut H) -> bool {
    h.try_run().is_ok()
}
