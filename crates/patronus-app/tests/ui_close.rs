//! Fermeture de la fenêtre : rien de perdu en silence. Une exécution en cours ou un brouillon non
//! enregistré demandent une confirmation ; sinon la fenêtre se ferme.

mod common;

use common::*;
use eframe::egui;
use patronus_core::run::Status;

/// Demande la fermeture (bouton de la barre de titre) ; rend vrai si l'application l'a empêchée.
fn close_is_cancelled(h: &mut H) -> bool {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();
    let cancelled = h.output().viewport_output[&egui::ViewportId::ROOT]
        .commands
        .iter()
        .any(|c| matches!(c, egui::ViewportCommand::CancelClose));
    h.step();
    cancelled
}

#[test]
fn nothing_to_lose_closes_at_once() {
    let dir = temp("fermer");
    script_task(&dir, "bonjour", "Bonjour contoso", "echo bonjour");
    let mut h = app(&dir);
    assert!(!close_is_cancelled(&mut h));
    assert!(!shows(&h, "Quitter Patronus ?"));
}

#[test]
fn running_run_asks_then_is_interrupted() {
    let dir = temp("fermer-en-cours");
    script_task(&dir, "long", "Longue northwind", "ping -n 30 127.0.0.1 >nul");
    let mut h = app(&dir);
    click(&mut h, "▶");
    wait_until(&mut h, "exécution en cours", |h| h.state().engine().running_count() == 1);
    assert!(close_is_cancelled(&mut h), "fermeture empêchée");
    assert!(shows(&h, "Quitter Patronus ?"));
    assert!(shows_containing(&h, "1 exécution est en cours"));
    click(&mut h, "Annuler");
    assert!(!shows(&h, "Quitter Patronus ?"));
    assert_eq!(h.state().engine().running_count(), 1, "annulé : elle continue");

    assert!(close_is_cancelled(&mut h));
    click(&mut h, "Arrêter et quitter");
    assert!(!close_is_cancelled(&mut h), "confirmée : la fermeture n'est plus empêchée");
    // Ce que fait eframe en fermant la fenêtre.
    eframe::App::on_exit(h.state_mut(), None);
    let run = h.state().engine().runs[0].clone();
    assert_eq!(run.statut, Status::Interrompue);
    assert!(run.resume.starts_with("Interrompue : Patronus a été fermé."), "{}", run.resume);
}

#[test]
fn unsaved_draft_asks() {
    let dir = temp("fermer-brouillon");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    replace_text(&mut h, "Nouvelle tâche", "Rapport fabrikam");
    assert!(close_is_cancelled(&mut h));
    assert!(shows_containing(&h, "« Rapport fabrikam » ne sont pas enregistrées"));
    click(&mut h, "Annuler");
    assert_eq!(h.state().draft().map(|t| t.nom), Some("Rapport fabrikam".into()), "brouillon gardé");
    assert!(close_is_cancelled(&mut h));
    click(&mut h, "Quitter");
    assert!(!close_is_cancelled(&mut h));
}
