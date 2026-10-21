//! États de référence figés : chaque version de Patronus doit relire sans perte ce que la version
//! 0.1 écrit (tâches, exécutions, `planning.json`), champs inconnus compris. Un test qui échoue ici
//! signale une incompatibilité : ne pas modifier le fichier de référence pour le faire passer.
//!
//! Limite connue : un champ inconnu à l'intérieur d'un déclencheur (`{"heure": …, "x": 1}`) n'est
//! pas gardé ; partout ailleurs il l'est.

use patronus_core::engine::Planning;
use patronus_core::run::{Run, Status};
use patronus_core::task::{Task, Trigger};
use serde_json::Value;

/// Relu puis réécrit : rien ne change.
fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(text: &str) -> (T, Value, Value) {
    let original: Value = serde_json::from_str(text).unwrap();
    let parsed: T = serde_json::from_str(text).unwrap();
    let again = serde_json::to_value(&parsed).unwrap();
    (parsed, original, again)
}

#[test]
fn reference_claude_task_v1() {
    let (t, original, again) = round_trip::<Task>(include_str!("data/tache-claude-v1.json"));
    assert_eq!(again, original, "tâche relue avec perte");
    assert_eq!(t.nom, "Revue des dépendances");
    assert!(matches!(&t.declencheurs[0], Trigger::Heure { rattrapage: true, .. }));
    assert!(t.validate().is_empty(), "{:?}", t.validate());
}

#[test]
fn reference_script_task_v1() {
    let (t, original, again) = round_trip::<Task>(include_str!("data/tache-script-v1.json"));
    assert_eq!(again, original, "tâche relue avec perte");
    assert!(!t.active);
    assert!(t.validate().is_empty(), "{:?}", t.validate());
}

#[test]
fn reference_run_v1() {
    let (r, original, again) = round_trip::<Run>(include_str!("data/execution-v1.json"));
    assert_eq!(again, original, "exécution relue avec perte");
    assert_eq!(r.statut, Status::BudgetAtteint);
    assert!(r.cout.is_some_and(|c| c.estime));
}

#[test]
fn reference_planning_v1() {
    let (p, original, again) = round_trip::<Planning>(include_str!("data/planning-v1.json"));
    assert_eq!(again, original, "planning.json relu avec perte");
    assert!(p.derniere_verification.is_some());
}
