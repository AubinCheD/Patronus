//! Réglages de l'application (`<données>\state.json`), versionnés comme l'état de Fawkes : version de
//! Patronus qui l'a écrit, copie de l'état précédent à un changement de version, champs inconnus
//! gardés, état de référence figé dans les tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use patronus_core::fsutil::{read_text, write_atomic};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::theme::ThemeChoice;

pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub saved_by: String,
    #[serde(default)]
    pub theme: ThemeChoice,
    /// Raccourcis qui diffèrent des valeurs par défaut.
    #[serde(default)]
    pub raccourcis: BTreeMap<String, String>,
    /// Jours de conservation des résultats lus.
    #[serde(default = "default_retention")]
    pub retention_jours: u32,
    /// Notifications à la fin d'une exécution (en plus du réglage de chaque tâche).
    #[serde(default = "yes")]
    pub notifications: bool,
    #[serde(default = "default_size")]
    pub taille_fenetre: [f32; 2],
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

fn default_retention() -> u32 {
    30
}
fn yes() -> bool {
    true
}
fn default_size() -> [f32; 2] {
    [1200.0, 760.0]
}

impl Default for State {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

/// L'état lu, et ce qu'il faut dire à l'utilisateur.
pub fn load(dir: &Path) -> (State, Vec<String>) {
    let path = dir.join("state.json");
    let mut problems = Vec::new();
    let text = match read_text(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (State::default(), problems),
        Err(e) => {
            problems.push(format!("Réglages illisibles ({}) : {e}. Valeurs par défaut.", path.display()));
            return (State::default(), problems);
        }
    };
    let st = match serde_json::from_str::<State>(&text) {
        Ok(st) => st,
        Err(e) => {
            let aside = dir.join("state.illisible.json");
            let _ = fs::copy(&path, &aside);
            problems
                .push(format!("Réglages illisibles ({e}) : mis de côté dans {}, valeurs par défaut.", aside.display()));
            return (State::default(), problems);
        }
    };
    let previous = if st.saved_by.is_empty() { "ancienne-version".to_string() } else { st.saved_by.clone() };
    if st.saved_by != env!("CARGO_PKG_VERSION") {
        let copy = dir.join(format!("state.avant-{previous}.json"));
        if !copy.exists() {
            let _ = fs::copy(&path, &copy);
        }
    }
    if st.version > STATE_VERSION {
        problems.push(format!(
            "Réglages écrits par une version plus récente de Patronus ({previous}) : relus au mieux, ce qui est inconnu est gardé."
        ));
    }
    (st, problems)
}

pub fn serialize(st: &State) -> Vec<u8> {
    let mut st = st.clone();
    st.version = STATE_VERSION.max(st.version);
    st.saved_by = env!("CARGO_PKG_VERSION").to_string();
    serde_json::to_vec_pretty(&st).unwrap_or_default()
}

pub fn save(dir: &Path, st: &State) -> std::io::Result<()> {
    write_atomic(&dir.join("state.json"), &serialize(st))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("patronus-app-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// L'état de référence de la version 1 : chaque version future doit le relire sans perte.
    #[test]
    fn reference_state_v1_still_loads() {
        let dir = temp("ref");
        fs::write(dir.join("state.json"), include_str!("../tests/data/state-v1.json")).unwrap();
        let (st, problems) = load(&dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(st.theme, ThemeChoice::Sombre);
        assert_eq!(st.raccourcis["boite"], "Ctrl+I");
        assert_eq!(st.retention_jours, 14);
        assert!(!st.notifications);
        assert_eq!(st.autres["champ_futur"], 42);
        let again: Value = serde_json::from_slice(&serialize(&st)).unwrap();
        assert_eq!(again["champ_futur"], 42, "champ inconnu gardé");
        assert_eq!(again["saved_by"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_change_keeps_a_copy() {
        let dir = temp("copy");
        fs::write(dir.join("state.json"), r#"{"version":1,"saved_by":"0.0.1"}"#).unwrap();
        load(&dir);
        assert!(dir.join("state.avant-0.0.1.json").exists());
    }

    #[test]
    fn unreadable_state_is_set_aside_and_said() {
        let dir = temp("bad");
        fs::write(dir.join("state.json"), "{").unwrap();
        let (st, problems) = load(&dir);
        assert_eq!(st, State::default());
        assert_eq!(problems.len(), 1);
        assert!(dir.join("state.illisible.json").exists());
    }

    #[test]
    fn missing_state_is_a_fresh_start() {
        let (st, problems) = load(&temp("missing"));
        assert!(problems.is_empty());
        assert_eq!(st.retention_jours, 30);
        assert!(st.notifications);
    }

    #[test]
    fn newer_state_is_reported() {
        let dir = temp("newer");
        fs::write(dir.join("state.json"), r#"{"version":99,"saved_by":"9.0.0"}"#).unwrap();
        let (_, problems) = load(&dir);
        assert!(problems[0].contains("plus récente"));
    }
}
