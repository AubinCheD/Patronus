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
    let run = &h.state().engine().runs[0];
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
