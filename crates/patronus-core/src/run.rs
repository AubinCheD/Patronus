//! Exécutions : une entrée de la boîte de résultats par passage d'une tâche, rangée dans
//! `<données>\executions\<id>\` (`run.json`, `journal.txt`, éventuellement `diff.patch`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fsutil::{read_text, write_atomic};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    EnCours,
    Reussie,
    Echouee,
    /// Arrêtée à la main.
    Arretee,
    DureeDepassee,
    BudgetAtteint,
    /// Départ prévu pendant que Patronus était fermé, non rattrapé.
    Manquee,
    /// Patronus s'est arrêté pendant l'exécution.
    Interrompue,
    /// La tâche n'a pas pu partir (invalide, dossier absent…).
    Impossible,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::EnCours => "en cours",
            Status::Reussie => "réussie",
            Status::Echouee => "échouée",
            Status::Arretee => "arrêtée",
            Status::DureeDepassee => "arrêtée : durée dépassée",
            Status::BudgetAtteint => "arrêtée : budget atteint",
            Status::Manquee => "manquée",
            Status::Interrompue => "interrompue",
            Status::Impossible => "impossible",
        }
    }

    pub fn is_ok(self) -> bool {
        self == Status::Reussie
    }
}

/// Coût d'une session Claude.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub tokens_entree: u64,
    pub tokens_sortie: u64,
    /// Équivalent API, en dollars, tel que rapporté par Claude.
    pub usd: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub tache: String,
    pub tache_nom: String,
    /// Ce qui l'a fait partir : « à la main », « 08:00 », « fichiers modifiés : … ».
    pub declencheur: String,
    pub debut: NaiveDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fin: Option<NaiveDateTime>,
    pub statut: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// Résumé court : la fin de la sortie, ou la réponse de Claude.
    #[serde(default)]
    pub resume: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cout: Option<Cost>,
    #[serde(default)]
    pub lu: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archive: bool,
    /// Dossier du worktree gardé (session Claude qui a produit un diff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    /// Session Claude de l'exécution, pour la reprendre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_claude: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub liens: Vec<String>,
    #[serde(flatten)]
    pub autres: BTreeMap<String, Value>,
}

impl Run {
    pub fn new(id: String, tache: &str, tache_nom: &str, declencheur: &str, debut: NaiveDateTime) -> Run {
        Run {
            id,
            tache: tache.to_string(),
            tache_nom: tache_nom.to_string(),
            declencheur: declencheur.to_string(),
            debut,
            fin: None,
            statut: Status::EnCours,
            code: None,
            resume: String::new(),
            cout: None,
            lu: false,
            archive: false,
            worktree: None,
            session_claude: None,
            liens: Vec::new(),
            autres: BTreeMap::new(),
        }
    }

    pub fn duration_secs(&self) -> Option<i64> {
        self.fin.map(|f| (f - self.debut).num_seconds())
    }
}

/// Les exécutions rangées dans `dir`.
#[derive(Clone)]
pub struct RunStore {
    pub dir: PathBuf,
}

impl RunStore {
    pub fn new(dir: PathBuf) -> Self {
        RunStore { dir }
    }

    pub fn run_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    pub fn journal_path(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("journal.txt")
    }

    pub fn diff_path(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("diff.patch")
    }

    /// Nouvel identifiant, triable par date : `20261021-080000-revue`, suffixé si déjà pris.
    pub fn new_id(&self, task_id: &str, at: NaiveDateTime) -> String {
        let base = format!("{}-{task_id}", at.format("%Y%m%d-%H%M%S"));
        let mut id = base.clone();
        let mut n = 2;
        while self.run_dir(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Crée le dossier et enregistre l'entrée.
    pub fn save(&self, run: &Run) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(run).map_err(std::io::Error::other)?;
        write_atomic(&self.run_dir(&run.id).join("run.json"), &json)
    }

    /// Toutes les entrées, les plus récentes d'abord, et les dossiers illisibles (signalés, jamais
    /// effacés). Une exécution restée « en cours » d'une session précédente devient « interrompue ».
    pub fn load_all(&self) -> (Vec<Run>, Vec<String>) {
        let mut runs = Vec::new();
        let mut problems = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else { return (runs, problems) };
        for e in entries.flatten().filter(|e| e.path().is_dir()) {
            let path = e.path().join("run.json");
            match read_text(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<Run>(&t).map_err(|e| e.to_string()))
            {
                Ok(mut run) => {
                    if run.statut == Status::EnCours {
                        run.statut = Status::Interrompue;
                        run.resume = "Patronus s'est arrêté pendant l'exécution.".into();
                        run.lu = false;
                        let _ = self.save(&run);
                    }
                    runs.push(run);
                }
                Err(err) => problems.push(format!("Exécution illisible ({}) : {err}", path.display())),
            }
        }
        runs.sort_by(|a, b| b.debut.cmp(&a.debut).then_with(|| b.id.cmp(&a.id)));
        (runs, problems)
    }

    /// Supprime les entrées archivées ou lues plus anciennes que `days` jours. Les non lues restent.
    pub fn prune(&self, runs: &mut Vec<Run>, days: u32, now: NaiveDateTime) -> usize {
        let limit = now - chrono::Duration::days(days as i64);
        let before = runs.len();
        runs.retain(|r| {
            let old = r.debut < limit && r.statut != Status::EnCours && (r.lu || r.archive);
            if old {
                let _ = fs::remove_dir_all(self.run_dir(&r.id));
            }
            !old
        });
        before - runs.len()
    }

    pub fn read_journal(&self, id: &str) -> String {
        read_lossy(&self.journal_path(id))
    }

    pub fn read_diff(&self, id: &str) -> Option<String> {
        let p = self.diff_path(id);
        p.exists().then(|| read_lossy(&p))
    }
}

pub fn now() -> NaiveDateTime {
    Local::now().naive_local().with_nanosecond(0).unwrap_or_else(|| Local::now().naive_local())
}

use chrono::Timelike;

/// Lit un fichier de sortie quel que soit son encodage (UTF-8 attendu, le reste remplacé).
pub fn read_lossy(path: &Path) -> String {
    fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// Les dernières lignes non vides d'une sortie, pour le résumé.
pub fn tail_summary(output: &str, lines: usize, max_chars: usize) -> String {
    let last: Vec<&str> = output.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let start = last.len().saturating_sub(lines);
    let s = last[start..].join("\n");
    if s.chars().count() > max_chars {
        let skip = s.chars().count() - max_chars;
        format!("…{}", s.chars().skip(skip).collect::<String>())
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;
    use chrono::NaiveDate;

    fn at(d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, 0, 0).unwrap()
    }

    #[test]
    fn runs_survive_a_restart_newest_first() {
        let store = RunStore::new(test_dir("runs"));
        for (d, statut) in [(20, Status::Reussie), (21, Status::Echouee)] {
            let id = store.new_id("revue", at(d, 8));
            let mut r = Run::new(id, "revue", "Revue", "08:00", at(d, 8));
            r.statut = statut;
            r.fin = Some(at(d, 9));
            store.save(&r).unwrap();
        }
        let (runs, problems) = store.load_all();
        assert!(problems.is_empty());
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].statut, Status::Echouee);
        assert_eq!(runs[1].duration_secs(), Some(3600));
    }

    #[test]
    fn ids_are_unique_and_sortable() {
        let store = RunStore::new(test_dir("ids"));
        let a = store.new_id("t", at(21, 8));
        assert_eq!(a, "20261021-080000-t");
        store.save(&Run::new(a.clone(), "t", "T", "", at(21, 8))).unwrap();
        assert_eq!(store.new_id("t", at(21, 8)), "20261021-080000-t-2");
    }

    #[test]
    fn a_run_left_running_becomes_interrupted() {
        let store = RunStore::new(test_dir("interrupted"));
        let r = Run::new("x".into(), "t", "T", "à la main", at(21, 8));
        store.save(&r).unwrap();
        let (runs, _) = store.load_all();
        assert_eq!(runs[0].statut, Status::Interrompue);
        assert!(runs[0].resume.contains("arrêté"));
        assert_eq!(store.load_all().0[0].statut, Status::Interrompue, "enregistré");
    }

    #[test]
    fn unreadable_runs_are_reported_not_deleted() {
        let dir = test_dir("unreadable");
        fs::create_dir_all(dir.join("casse")).unwrap();
        fs::write(dir.join("casse").join("run.json"), "{").unwrap();
        let (runs, problems) = RunStore::new(dir.clone()).load_all();
        assert!(runs.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(dir.join("casse").join("run.json").exists());
    }

    #[test]
    fn prune_keeps_unread_and_recent() {
        let store = RunStore::new(test_dir("prune"));
        let mut runs = Vec::new();
        for (id, d, lu) in [("vieux-lu", 1, true), ("vieux-non-lu", 1, false), ("recent-lu", 20, true)] {
            let mut r = Run::new(id.into(), "t", "T", "", at(d, 8));
            r.statut = Status::Reussie;
            r.lu = lu;
            store.save(&r).unwrap();
            runs.push(r);
        }
        assert_eq!(store.prune(&mut runs, 7, at(21, 8)), 1);
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["vieux-non-lu", "recent-lu"]);
        assert!(!store.run_dir("vieux-lu").exists());
    }

    #[test]
    fn summary_is_the_tail() {
        assert_eq!(tail_summary("a\n\nb\nc\n\n", 2, 100), "b\nc");
        assert_eq!(tail_summary("abcdef", 1, 3), "…def");
    }
}
