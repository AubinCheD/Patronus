//! Boîte de résultats, par de vrais clics : recherche, tout marquer comme lu ; dupliquer une tâche.

mod common;

use std::path::Path;

use common::*;
use patronus_core::run::{Run, RunStore, Status, now};

/// Deux résultats non lus : un rapport fabrikam réussi, une revue contoso échouée.
fn two_runs(data: &Path) {
    let store = RunStore::new(data.join("executions"));
    for (id, tache, nom, resume, statut) in [
        ("20261022-080000-rapport", "rapport", "Rapport fabrikam", "3 tickets ouverts", Status::Reussie),
        ("20261022-090000-revue", "revue", "Revue contoso", "Compilation impossible", Status::Echouee),
    ] {
        let mut run = Run::new(id.into(), tache, nom, "à la main", now());
        run.statut = statut;
        run.resume = resume.into();
        store.save(&run).unwrap();
    }
}

#[test]
fn search_filters_the_inbox() {
    let dir = temp("recherche");
    two_runs(&dir);
    let mut h = app(&dir);
    wait_until(&mut h, "deux résultats", |h| shows(h, "Rapport fabrikam") && shows(h, "Revue contoso"));
    type_in(&mut h, "Rechercher (tâche, résumé, déclencheur)", "compilation");
    wait_until(&mut h, "un seul résultat", |h| !shows(h, "Rapport fabrikam") && shows(h, "Revue contoso"));
    replace_text(&mut h, "compilation", "rien de tel");
    wait_until(&mut h, "aucun résultat", |h| shows(h, "Aucun résultat avec ces filtres."));
}

#[test]
fn mark_all_read() {
    let dir = temp("tout-lu");
    two_runs(&dir);
    let mut h = app(&dir);
    assert_eq!(h.state().engine().unread_count(), 2);
    click(&mut h, "Tout marquer comme lu");
    wait_until(&mut h, "tout lu", |h| h.state().engine().unread_count() == 0);
    let reread = RunStore::new(dir.join("executions")).load_all().0;
    assert!(reread.iter().all(|r| r.lu), "enregistré");
}

#[test]
fn duplicate_opens_an_unsaved_copy() {
    let dir = temp("dupliquer");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    click(&mut h, "✏");
    click(&mut h, "Dupliquer");
    wait_until(&mut h, "copie ouverte", |h| h.state().draft().is_some_and(|t| t.nom == "Bonjour contoso (copie)"));
    assert!(shows(&h, "Nouvelle tâche"), "nouvelle tâche, pas encore enregistrée");
    click(&mut h, "Enregistrer");
    wait_until(&mut h, "deux tâches", |h| h.state().engine().loaded.len() == 2);
    let names: Vec<String> =
        h.state().engine().loaded.iter().filter_map(|l| l.task.as_ref().ok().map(|t| t.nom.clone())).collect();
    assert!(names.contains(&"Bonjour contoso (copie)".to_string()), "{names:?}");
}
