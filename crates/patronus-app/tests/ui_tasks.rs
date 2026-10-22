//! Liste des tâches, par de vrais clics : lancer, mettre en pause.

mod common;

use common::*;
use patronus_core::run::Status;
use patronus_core::task::TaskStore;

#[test]
fn run_button_launches_the_task() {
    let dir = temp("lancer");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour contoso");
    let mut h = app(&dir);
    click(&mut h, "▶");
    wait_until(&mut h, "exécution terminée", |h| {
        h.state().engine().runs.first().is_some_and(|r| r.statut != Status::EnCours)
    });
    let run = h.state().engine().runs[0].clone();
    assert_eq!(run.statut, Status::Reussie, "{}", run.resume);
    assert!(run.resume.contains("bonjour contoso"));
    assert_eq!(run.declencheur, "à la main");
    wait_until(&mut h, "résultat affiché", |h| shows_containing(h, "réussie"));
}

#[test]
fn checkbox_pauses_the_task() {
    let dir = temp("pause");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    click(&mut h, "Active (sinon en pause)");
    let store = TaskStore::new(dir.join("taches"));
    wait_until(&mut h, "tâche en pause enregistrée", |_| {
        store.load_all().first().and_then(|l| l.task.as_ref().ok()).is_some_and(|t| !t.active)
    });
}

#[test]
fn clicking_a_row_filters_the_inbox() {
    let dir = temp("filtre");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    assert_eq!(h.state().inbox_filter(), None);
    click(&mut h, "Bonjour contoso");
    wait_until(&mut h, "boîte filtrée sur la tâche", |h| h.state().inbox_filter() == Some("bonjour"));
}

/// Au repos, la fenêtre ne se redessine pas en continu (le titre envoyé à chaque image la
/// redessinait sans fin).
#[test]
fn idle_window_settles() {
    let dir = temp("repos");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    assert!(settles(&mut h));
}

#[test]
fn general_pause_button() {
    let dir = temp("pause-generale");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    click(&mut h, "Tout mettre en pause");
    wait_until(&mut h, "pause affichée", |h| shows(h, "En pause : aucun départ automatique"));
    assert!(h.state().engine().paused());
    click(&mut h, "▶");
    wait_until(&mut h, "lancement à la main malgré la pause", |h| {
        h.state().engine().runs.first().is_some_and(|r| r.statut == Status::Reussie)
    });
    click(&mut h, "Reprendre");
    wait_until(&mut h, "reprise", |h| !shows(h, "En pause : aucun départ automatique"));
    assert!(!h.state().engine().paused());
}

/// Palette : flèches haut et bas pour choisir, Entrée pour lancer.
#[test]
fn palette_with_arrow_keys() {
    use eframe::egui::{Key, Modifiers};
    let dir = temp("palette-fleches");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    let open = |h: &mut H| {
        h.key_press_modifiers(Modifiers::COMMAND, Key::P);
        h.step();
        type_in(h, "Commande ou tâche…", "contoso");
    };
    open(&mut h);
    // Lancer, Mettre en pause, Modifier, Dupliquer : deux crans vers le bas, « Modifier ».
    for _ in 0..2 {
        h.key_press(Key::ArrowDown);
        h.step();
    }
    h.key_press(Key::Enter);
    wait_until(&mut h, "tâche ouverte", |h| h.state().draft().is_some_and(|t| t.nom == "Bonjour contoso"));
    // Vers le haut depuis la première : la dernière, « Dupliquer ».
    open(&mut h);
    h.key_press(Key::ArrowUp);
    h.step();
    h.key_press(Key::Enter);
    wait_until(&mut h, "copie ouverte", |h| h.state().draft().is_some_and(|t| t.nom == "Bonjour contoso (copie)"));
}
