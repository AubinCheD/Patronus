//! Actions sur un résultat : appliquer le diff d'une session, supprimer son worktree, reprendre la
//! session. Sur de vrais dépôts git temporaires (fictifs).

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{NaiveDate, NaiveDateTime};
use patronus_core::actions::{Launch, apply, check_apply, resume_command};
use patronus_core::claude::{create_worktree, worktree_diff};
use patronus_core::engine::Engine;
use patronus_core::run::{Run, RunStore, Status};

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 22).unwrap().and_hms_opt(9, 0, 0).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("patronus-actions-{name}-{}", std::process::id()));
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Un dépôt `contoso-web` et le diff d'une session faite dans un worktree : README modifié, une
/// page ajoutée. Rend (dépôt, dossier de données, identifiant de l'exécution).
fn session_with_diff(dir: &Path) -> (PathBuf, PathBuf, String) {
    let repo = dir.join("contoso-web");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "contoso\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "Premier commit"]);
    let data = dir.join("donnees");
    let id = "20261022-080000-doc".to_string();
    let wt = data.join("worktrees").join(&id);
    let base = create_worktree(&repo, "main", &wt).unwrap();
    std::fs::write(wt.join("README.md"), "contoso\nDocumentation à jour.\n").unwrap();
    std::fs::write(wt.join("GUIDE.md"), "Guide de démarrage.\n").unwrap();
    let diff = worktree_diff(&wt, &base).unwrap();
    let store = RunStore::new(data.join("executions"));
    let mut run = Run::new(id.clone(), "doc", "Doc à jour", "à la main", now());
    run.statut = Status::Reussie;
    run.worktree = Some(wt);
    run.depot = Some(repo.clone());
    store.save(&run).unwrap();
    std::fs::write(store.diff_path(&id), diff).unwrap();
    (repo, data, id)
}

#[test]
fn diff_is_applied_to_the_working_copy_only() {
    let dir = temp("appliquer");
    let (repo, data, id) = session_with_diff(&dir);
    let patch = RunStore::new(data.join("executions")).diff_path(&id);
    let plan = check_apply(&repo, &patch).unwrap();
    assert_eq!(plan.branch, "main");
    assert_eq!(plan.files, ["GUIDE.md", "README.md"]);
    apply(&repo, &patch).unwrap();
    assert!(std::fs::read_to_string(repo.join("README.md")).unwrap().contains("Documentation à jour."));
    assert!(repo.join("GUIDE.md").exists());
    assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "", "rien dans l'index");
    assert_eq!(git(&repo, &["log", "--oneline"]).lines().count(), 1, "aucun commit");
}

#[test]
fn diverged_copy_is_refused() {
    let dir = temp("diverge");
    let (repo, data, id) = session_with_diff(&dir);
    std::fs::write(repo.join("README.md"), "contoso, modifié entre-temps\n").unwrap();
    let err = check_apply(&repo, &RunStore::new(data.join("executions")).diff_path(&id)).unwrap_err();
    assert!(err.contains("ne s'applique pas"), "{err}");
}

/// Par le moteur : appliquer note la date dans l'entrée ; supprimer le worktree le retire du disque
/// et de git, le diff reste.
#[test]
fn engine_applies_and_drops_the_worktree() {
    let dir = temp("moteur");
    let (repo, data, id) = session_with_diff(&dir);
    let mut e = Engine::open(&data, now());
    let plan = e.diff_plan(&id).unwrap();
    assert_eq!(plan.files.len(), 2);
    e.apply_diff(&id, now()).unwrap();
    assert!(e.runs.iter().find(|r| r.id == id).unwrap().diff_applique.is_some());
    assert!(e.apply_diff(&id, now()).unwrap_err().contains("déjà appliqué"));
    e.drop_worktree(&id).unwrap();
    assert!(!data.join("worktrees").join(&id).exists());
    assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 1);
    assert!(e.runs.iter().find(|r| r.id == id).unwrap().worktree.is_none());
    assert!(e.store.read_diff(&id).is_some(), "le diff reste dans l'entrée");
    let reopened = Engine::open(&data, now());
    assert!(reopened.runs[0].diff_applique.is_some(), "enregistré");
}

#[test]
fn resume_command_prefers_windows_terminal() {
    let dir = PathBuf::from(r"C:\donnees\worktrees\a;b");
    let claude = PathBuf::from(r"C:\outils\claude.exe");
    let wt = PathBuf::from(r"C:\WindowsApps\wt.exe");
    let l: Launch = resume_command(&dir, "abc-123", &claude, Some(&wt));
    assert_eq!(l.program, wt.as_os_str());
    assert_eq!(l.args[1], r"C:\donnees\worktrees\a\;b", "le ; s'échappe pour wt");
    let expected: Vec<std::ffi::OsString> = vec![claude.as_os_str().to_owned(), "--resume".into(), "abc-123".into()];
    assert_eq!(l.args[2..], expected[..]);
    assert!(!l.new_console);
    let l = resume_command(&dir, "abc-123", &claude, None);
    assert_eq!(l.program, "powershell.exe");
    assert!(l.new_console);
    assert_eq!(l.args[2], r"& 'C:\outils\claude.exe' --resume 'abc-123'");
    assert_eq!(l.current_dir, dir);
}
