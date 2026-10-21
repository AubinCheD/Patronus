//! Rétention et worktrees : sur de vrais dépôts git temporaires (fictifs).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use patronus_core::claude::{WorktreeInfo, create_worktree};
use patronus_core::engine::{Engine, Options};
use patronus_core::run::{Run, RunStore, Status};

fn at(d: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(10, 0, 0).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("patronus-wt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Essai", "-c", "user.email=essai@contoso.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} : {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Un dépôt fictif `fabrikam-web`, branche `main`, un commit.
fn repo(dir: &Path) -> PathBuf {
    let r = dir.join("fabrikam-web");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    std::fs::write(r.join("README.md"), "fabrikam\n").unwrap();
    git(&r, &["add", "."]);
    git(&r, &["commit", "-q", "-m", "Premier commit"]);
    r
}

fn run(id: &str, day: u32, statut: Status, lu: bool) -> Run {
    let mut r = Run::new(id.into(), "revue", "Revue", "à la main", at(day));
    r.statut = statut;
    r.fin = Some(at(day));
    r.lu = lu;
    r
}

/// Fait un tour du moteur (la maintenance a lieu au premier tour, dans le fil du moteur).
fn first_tick(e: &mut Engine, day: u32) {
    e.tick(at(day));
}

#[test]
fn retention_setting_is_applied() {
    let data = temp("retention");
    let store = RunStore::new(data.join("executions"));
    for (id, day) in [("vieux", 1), ("recent", 15)] {
        store.save(&run(id, day, Status::Reussie, true)).unwrap();
    }
    // Réglée à un an : rien n'est supprimé (le premier jet supprimait à 30 jours, toujours).
    let mut e = Engine::open_opts(&data, at(21), Options { retention_days: 365, ..Options::default() });
    first_tick(&mut e, 21);
    assert_eq!(e.runs.len(), 2);
    // Réglée à 10 jours : la plus vieille part au tour suivant.
    e.set_retention(10);
    e.tick(at(21));
    assert_eq!(e.runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["recent"]);
    assert!(!data.join("executions").join("vieux").exists());
}

#[test]
fn pruned_entry_removes_its_worktree() {
    let dir = temp("prune");
    let r = repo(&dir);
    let data = dir.join("donnees");
    let wt = data.join("worktrees").join("vieux");
    create_worktree(&r, "main", &wt).unwrap();
    let mut old = run("vieux", 1, Status::Reussie, true);
    old.worktree = Some(wt.clone());
    old.depot = Some(r.clone());
    RunStore::new(data.join("executions")).save(&old).unwrap();
    let mut e = Engine::open_opts(&data, at(21), Options { retention_days: 7, ..Options::default() });
    first_tick(&mut e, 21);
    assert!(e.runs.is_empty());
    assert!(!wt.exists(), "worktree supprimé avec son entrée");
    assert_eq!(git(&r, &["worktree", "list"]).lines().count(), 1, "métadonnées du worktree nettoyées");
    assert!(e.problems.is_empty(), "{:?}", e.problems);
}

/// Une exécution coupée (Patronus arrêté) a laissé son worktree : à la réouverture, le diff est
/// récupéré s'il y a des changements, et le worktree vide est supprimé.
#[test]
fn orphan_worktrees_are_reconciled() {
    let dir = temp("orphelins");
    let r = repo(&dir);
    let data = dir.join("donnees");
    let store = RunStore::new(data.join("executions"));
    for (id, write) in [("avec-changements", true), ("sans-changement", false)] {
        let wt = data.join("worktrees").join(id);
        let base = create_worktree(&r, "main", &wt).unwrap();
        let info = WorktreeInfo { chemin: wt.clone(), depot: r.clone(), base };
        std::fs::create_dir_all(store.run_dir(id)).unwrap();
        std::fs::write(store.run_dir(id).join("worktree.json"), serde_json::to_string(&info).unwrap()).unwrap();
        if write {
            std::fs::write(wt.join("NOTES.md"), "Proposition interrompue.\n").unwrap();
        }
        // « en cours » sans processus vivant : déclarée interrompue à l'ouverture.
        store.save(&Run::new(id.into(), "revue", "Revue", "à la main", at(21))).unwrap();
    }
    let mut e = Engine::open(&data, at(21));
    first_tick(&mut e, 21);
    let kept = e.runs.iter().find(|x| x.id == "avec-changements").unwrap();
    assert_eq!(kept.statut, Status::Interrompue);
    assert_eq!(kept.worktree.as_deref(), Some(data.join("worktrees").join("avec-changements").as_path()));
    assert!(kept.resume.contains("Diff récupéré"), "{}", kept.resume);
    let diff = store.read_diff("avec-changements").expect("diff enregistré");
    assert!(diff.contains("+Proposition interrompue."));
    assert!(!data.join("worktrees").join("sans-changement").exists(), "worktree vide supprimé");
    assert_eq!(git(&r, &["status", "--porcelain"]), "", "dépôt de l'utilisateur intact");
    // Rien de plus au tour suivant : le worktree gardé n'est pas rapproché deux fois.
    std::thread::sleep(Duration::from_millis(10));
    e.tick(at(21));
    assert!(e.problems.is_empty(), "{:?}", e.problems);
}

#[test]
fn unknown_worktree_is_left_and_reported() {
    let dir = temp("inconnu");
    let r = repo(&dir);
    let data = dir.join("donnees");
    let wt = data.join("worktrees").join("mystere");
    create_worktree(&r, "main", &wt).unwrap();
    let mut e = Engine::open(&data, at(21));
    first_tick(&mut e, 21);
    assert!(wt.exists(), "laissé en place");
    assert!(e.problems.iter().any(|p| p.contains("sans exécution connue")), "{:?}", e.problems);
}
