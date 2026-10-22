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
    /// Nombre de tours maximal atteint (par Claude Code ou par Patronus).
    ToursAtteints,
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
            Status::ToursAtteints => "arrêtée : nombre de tours atteint",
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
    /// Estimé par Patronus d'après l'usage annoncé (session arrêtée avant son résultat).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub estime: bool,
}

/// Un processus : numéro et heure de création (un numéro seul peut être redonné à un autre).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    pub cree: u64,
}

impl Owner {
    pub fn current() -> Owner {
        let (pid, cree) = crate::winproc::current();
        Owner { pid, cree }
    }

    pub fn is_alive(&self) -> bool {
        crate::winproc::is_alive(self.pid, self.cree)
    }

    /// Un autre processus, encore vivant.
    pub fn is_other_live_process(&self) -> bool {
        *self != Owner::current() && self.is_alive()
    }
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
    /// Processus qui mène l'exécution (fenêtre, ligne de commande ou Planificateur) : tant qu'il vit,
    /// une exécution « en cours » n'est pas déclarée interrompue par un autre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proprietaire: Option<Owner>,
    /// Dépôt de la tâche au moment de l'exécution (diff à appliquer, worktree à nettoyer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depot: Option<PathBuf>,
    /// Diff appliqué dans la copie de travail (à la demande de l'utilisateur), et quand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_applique: Option<NaiveDateTime>,
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
            proprietaire: None,
            depot: None,
            diff_applique: None,
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
                    // Une exécution menée par un autre processus encore vivant continue : elle n'est pas
                    // interrompue (la fenêtre et la ligne de commande partagent le dossier).
                    let elsewhere = run.proprietaire.is_some_and(|o| o.is_other_live_process());
                    if run.statut == Status::EnCours && !elsewhere {
                        // Relue une fois le processus su mort : il a pu noter sa fin entre-temps.
                        if let Some(again) = read_text(&path).ok().and_then(|t| serde_json::from_str::<Run>(&t).ok()) {
                            run = again;
                        }
                    }
                    if run.statut == Status::EnCours && !elsewhere {
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

    /// Relit une entrée.
    pub fn load_one(&self, id: &str) -> Option<Run> {
        let text = read_text(&self.run_dir(id).join("run.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Identifiants des entrées présentes sur le disque.
    pub fn ids(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        entries
            .flatten()
            .filter(|e| e.path().join("run.json").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    /// Supprime les entrées archivées ou lues plus anciennes que `days` jours (les non lues restent),
    /// et les rend : leur worktree éventuel est à supprimer aussi.
    pub fn prune(&self, runs: &mut Vec<Run>, days: u32, now: NaiveDateTime) -> Vec<Run> {
        let limit = now - chrono::Duration::days(days as i64);
        let (old, keep): (Vec<Run>, Vec<Run>) = std::mem::take(runs)
            .into_iter()
            .partition(|r| r.debut < limit && r.statut != Status::EnCours && (r.lu || r.archive));
        for r in &old {
            let _ = fs::remove_dir_all(self.run_dir(&r.id));
        }
        *runs = keep;
        old
    }

    pub fn read_journal(&self, id: &str) -> String {
        read_lossy(&self.journal_path(id))
    }

    /// La fin du journal (`max_bytes` au plus), à partir d'une ligne entière.
    pub fn read_journal_tail(&self, id: &str, max_bytes: u64) -> String {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = fs::File::open(self.journal_path(id)) else { return String::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let start = len.saturating_sub(max_bytes);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
            return String::new();
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        if start == 0 { text } else { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()) }
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

/// Liens d'une entrée : 10 au plus.
pub const MAX_LINKS: usize = 10;

/// Les liens `http://` et `https://` d'un texte, dans l'ordre, sans doublon, `max` au plus. La
/// ponctuation qui suit un lien (fin de phrase, parenthèse d'un lien Markdown) n'en fait pas partie.
pub fn links(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while out.len() < max {
        let Some(i) = ["https://", "http://"].iter().filter_map(|s| rest.find(s)).min() else { break };
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '{' | '}'))
            .unwrap_or(tail.len());
        let mut url = &tail[..end];
        loop {
            let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            let unbalanced = |close: char, open: char| {
                trimmed.ends_with(close) && trimmed.matches(open).count() < trimmed.matches(close).count()
            };
            let trimmed =
                if unbalanced(')', '(') || unbalanced(']', '[') { &trimmed[..trimmed.len() - 1] } else { trimmed };
            if trimmed.len() == url.len() {
                break;
            }
            url = trimmed;
        }
        let has_host = url.split_once("://").is_some_and(|(_, h)| !h.is_empty());
        if has_host && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        rest = &tail[end..];
    }
    out
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
        assert_eq!(store.prune(&mut runs, 7, at(21, 8)).len(), 1);
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["vieux-non-lu", "recent-lu"]);
        assert!(!store.run_dir("vieux-lu").exists());
    }

    #[test]
    fn summary_is_the_tail() {
        assert_eq!(tail_summary("a\n\nb\nc\n\n", 2, 100), "b\nc");
        assert_eq!(tail_summary("abcdef", 1, 3), "…def");
    }

    #[test]
    fn links_are_extracted() {
        let text = "PR ouverte : https://contoso.com/pr/12. Voir [la CI](https://ci.contoso.com/b/7) \
                    et (https://fabrikam.com/doc_(v2)), encore https://contoso.com/pr/12 et http:// rien.\n\
                    \"https://northwind.com/a?b=1&c=2\"";
        assert_eq!(
            links(text, MAX_LINKS),
            [
                "https://contoso.com/pr/12",
                "https://ci.contoso.com/b/7",
                "https://fabrikam.com/doc_(v2)",
                "https://northwind.com/a?b=1&c=2",
            ]
        );
        let many: String = (0..20).map(|i| format!("https://contoso.com/{i} ")).collect();
        assert_eq!(links(&many, MAX_LINKS).len(), 10);
        assert!(links("ftp://contoso.com file://c/x", MAX_LINKS).is_empty(), "http(s) seulement");
    }
}
