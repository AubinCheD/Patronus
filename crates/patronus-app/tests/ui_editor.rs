//! Éditeur de tâche, par de vrais clics.

mod common;

use common::*;
use egui_kittest::kittest::Queryable;

/// Une session Claude travaille d'office dans un worktree dédié.
#[test]
fn claude_session_gets_a_worktree_by_default() {
    let dir = temp("editeur-claude");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    click(&mut h, "Session Claude non interactive");
    let draft = h.state().draft().expect("éditeur ouvert");
    assert!(draft.action.claude.is_some());
    assert_eq!(draft.action.worktree.map(|w| w.branche).as_deref(), Some("main"));
}

/// Un brouillon survit au passage par la boîte, et se reprend d'un clic.
#[test]
fn draft_survives_switching_to_the_inbox() {
    let dir = temp("brouillon");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    replace_text(&mut h, "Nouvelle tâche", "Point contoso");
    click(&mut h, "Boîte de résultats");
    click(&mut h, "✏ Reprendre « Point contoso » (modifiée)");
    assert_eq!(h.state().draft().unwrap().nom, "Point contoso");
}

/// Ouvrir une nouvelle tâche par-dessus un brouillon modifié demande d'abord ; « Revenir au
/// brouillon » le garde, « Abandonner » le remplace.
#[test]
fn abandoning_a_draft_asks_first() {
    let dir = temp("abandon");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    replace_text(&mut h, "Nouvelle tâche", "Revue fabrikam");
    click(&mut h, "+ Nouvelle tâche");
    wait_until(&mut h, "confirmation", |h| shows(h, "Modifications non enregistrées"));
    click(&mut h, "Revenir au brouillon");
    assert_eq!(h.state().draft().unwrap().nom, "Revue fabrikam");
    click(&mut h, "Fermer");
    click(&mut h, "Abandonner les modifications");
    wait_until(&mut h, "éditeur fermé", |h| h.state().draft().is_none());
}

/// Une tâche enregistrée n'est plus « modifiée » : la fermer ne demande rien.
#[test]
fn saved_task_closes_without_asking() {
    let dir = temp("enregistree");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    replace_text(&mut h, "Nouvelle tâche", "Tests northwind");
    click(&mut h, "Enregistrer quand même");
    click(&mut h, "Fermer");
    wait_until(&mut h, "éditeur fermé", |h| h.state().draft().is_none());
    assert!(!shows(&h, "Modifications non enregistrées"));
}

/// F5 (lancer la tâche choisie) ne fait rien pendant une saisie.
#[test]
fn f5_while_typing_does_not_launch() {
    let dir = temp("f5");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    click(&mut h, "✏");
    let field = h.get_by(|n| {
        n.role() == eframe::egui::accesskit::Role::TextInput && n.value().as_deref() == Some("Bonjour contoso")
    });
    field.focus();
    h.step();
    h.key_press(eframe::egui::Key::F5);
    h.step();
    h.step();
    assert!(h.state().engine().runs.is_empty(), "lancée pendant la saisie");
}

/// Hors saisie, F5 lance la tâche choisie (la contrepartie du test précédent).
#[test]
fn f5_launches_the_selected_task() {
    let dir = temp("f5-lance");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    h.key_press(eframe::egui::Key::F5);
    wait_until(&mut h, "tâche lancée par F5", |h| !h.state().engine().runs.is_empty());
}

/// Une nouvelle tâche part d'un modèle ; le dépôt déjà saisi est gardé.
#[test]
fn task_from_template() {
    let dir = temp("modele");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    type_in(&mut h, r"facultatif : C:\dev\contoso-api", r"C:\dev\northwind");
    click(&mut h, "Point du matin");
    let draft = h.state().draft().unwrap();
    assert_eq!(draft.nom, "Point du matin");
    assert_eq!(draft.depot.as_deref(), Some(std::path::Path::new(r"C:\dev\northwind")));
    assert!(!draft.declencheurs.is_empty());
    assert!(draft.validate().is_empty(), "{:?}", draft.validate());
    // Le modèle choisi devient la base : tant que rien n'est modifié, on peut encore en changer.
    click(&mut h, "Tests réguliers");
    assert_eq!(h.state().draft().unwrap().nom, "Tests réguliers");
}
