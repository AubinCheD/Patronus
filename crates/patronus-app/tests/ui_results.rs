//! Actions sur un résultat, par de vrais clics : appliquer le diff d'une session (après
//! confirmation), supprimer son worktree.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use patronus_core::claude::{create_worktree, worktree_diff};
use patronus_core::run::{Run, RunStore, Status, now};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Essai", "-c", "user.email=essai@contoso.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} : {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Un dépôt `fabrikam-shop` et une session terminée qui a ajouté `GUIDE.md` dans son worktree.
fn finished_session(dir: &Path) -> (PathBuf, PathBuf) {
    let repo = dir.join("fabrikam-shop");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "fabrikam\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "Premier commit"]);
    let data = dir.join("donnees");
    let id = "20261022-080000-doc";
    let wt = data.join("worktrees").join(id);
    let base = create_worktree(&repo, "main", &wt).unwrap();
    std::fs::write(wt.join("GUIDE.md"), "Guide de démarrage.\n").unwrap();
    let diff = worktree_diff(&wt, &base).unwrap();
    let store = RunStore::new(data.join("executions"));
    let mut run = Run::new(id.into(), "doc", "Doc à jour", "à la main", now());
    run.statut = Status::Reussie;
    run.resume = "Guide ajouté.".into();
    run.worktree = Some(wt);
    run.depot = Some(repo.clone());
    store.save(&run).unwrap();
    std::fs::write(store.diff_path(id), diff).unwrap();
    (repo, data)
}

#[test]
fn apply_diff_asks_then_applies() {
    let dir = temp("appliquer-diff");
    let (repo, data) = finished_session(&dir);
    let mut h = app(&data);
    click(&mut h, "Appliquer le diff…");
    wait_until(&mut h, "confirmation", |h| shows(h, "Appliquer le diff"));
    assert!(shows(&h, "GUIDE.md"), "les fichiers touchés sont montrés");
    assert!(!repo.join("GUIDE.md").exists(), "rien avant la confirmation");
    click(&mut h, "Appliquer");
    wait_until(&mut h, "diff appliqué", |_| repo.join("GUIDE.md").exists());
    assert_eq!(git(&repo, &["log", "--oneline"]).lines().count(), 1, "aucun commit");
    wait_until(&mut h, "mention dans le résultat", |h| shows_containing(h, "Diff appliqué le"));
}

#[test]
fn worktree_is_dropped_after_confirmation() {
    let dir = temp("supprimer-worktree");
    let (repo, data) = finished_session(&dir);
    let wt = data.join("worktrees").join("20261022-080000-doc");
    let mut h = app(&data);
    click(&mut h, "Supprimer le worktree…");
    click(&mut h, "Annuler");
    assert!(wt.exists(), "annulé : rien de supprimé");
    click(&mut h, "Supprimer le worktree…");
    click(&mut h, "Supprimer");
    wait_until(&mut h, "worktree supprimé", |_| !wt.exists());
    assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 1);
    wait_until(&mut h, "entrée sans worktree", |h| {
        h.state().engine().runs[0].worktree.is_none() && !shows(h, "Supprimer le worktree…")
    });
}

#[test]
fn links_are_shown() {
    let dir = temp("liens");
    let data = dir.join("donnees");
    let store = RunStore::new(data.join("executions"));
    let mut run = Run::new("20261022-080000-rapport".into(), "rapport", "Rapport contoso", "à la main", now());
    run.statut = Status::Reussie;
    run.resume = "Rapport prêt.".into();
    run.liens = vec!["https://contoso.com/rapport/12".into(), "https://contoso.com/ci/3".into()];
    store.save(&run).unwrap();
    let mut h = app(&data);
    wait_until(&mut h, "liens affichés", |h| {
        shows(h, "https://contoso.com/rapport/12") && shows(h, "https://contoso.com/ci/3")
    });
}
