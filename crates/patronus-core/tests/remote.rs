//! Déclencheur « branche distante », sur de vrais dépôts git temporaires (fictifs) : un dépôt
//! « distant » et son clone. Le distant avance par des commits faits directement chez lui (aucun
//! push).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, NaiveDateTime};
use patronus_core::engine::{Engine, Executor, Options};
use patronus_core::exec::{Context, Outcome};
use patronus_core::run::Status;
use patronus_core::task::{RemoteBranch, Script, Task, Trigger};

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 21).unwrap().and_hms_opt(10, 0, 0).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("patronus-distant-{name}-{}", std::process::id()));
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

fn commit(repo: &Path, file: &str, message: &str) -> String {
    std::fs::write(repo.join(file), message).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// Un dépôt « distant » `northwind-api` (branche main, un commit) et son clone `depot`.
fn repos(dir: &Path) -> (PathBuf, PathBuf) {
    let remote = dir.join("northwind-api");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "-b", "main"]);
    commit(&remote, "README.md", "Premier commit");
    let out = Command::new("git").arg("clone").arg("-q").arg(&remote).arg(dir.join("depot")).output().unwrap();
    assert!(out.status.success());
    (remote, dir.join("depot"))
}

/// Faux exécuteur : garde le contexte de chaque départ.
fn recorder() -> (Executor, Arc<Mutex<Vec<Context>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let f: Executor = Arc::new(move |_, _, ctx, _, _| {
        s.lock().unwrap().push(ctx.clone());
        Outcome {
            status: Status::Reussie,
            code: Some(0),
            summary: "ok".into(),
            cost: None,
            worktree: None,
            session: None,
        }
    });
    (f, seen)
}

fn engine(data: &Path, exec: Executor) -> Engine {
    Engine::open_opts(data, now(), Options { executor: Some(exec), ..Options::default() })
}

fn branch_task(depot: &Path, remote: &str) -> Task {
    let mut t = Task::new_script("Doc à jour après une fusion");
    t.depot = Some(depot.to_path_buf());
    t.action.script = Some(Script { commande: "echo x".into(), ..Default::default() });
    let mut b = RemoteBranch::new("main");
    b.distant = remote.into();
    t.declencheurs = vec![Trigger::Branche { branche_distante: b }];
    t
}

/// Relève les branches et attend que le relevé soit revenu (`done`).
fn poll(e: &mut Engine, mut done: impl FnMut(&Engine) -> bool) {
    e.poll_branches_now();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        e.tick(now());
        if done(e) {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("relevé non revenu ; problèmes : {:?}", e.problems);
}

fn known(data: &Path) -> Option<String> {
    let text = std::fs::read_to_string(data.join("planning.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["branches"]["doc#0"].as_str().map(str::to_string)
}

#[test]
fn first_read_does_not_trigger_then_a_new_commit_does() {
    let dir = temp("avance");
    let (remote, depot) = repos(&dir);
    let data = dir.join("donnees");
    let (exec, seen) = recorder();
    let mut e = engine(&data, exec);
    e.save_task(Some("doc"), &branch_task(&depot, "origin"), now()).unwrap();
    let first = git(&remote, &["rev-parse", "HEAD"]);
    poll(&mut e, |_| known(&data).is_some());
    assert_eq!(known(&data).as_deref(), Some(first.as_str()));
    assert!(seen.lock().unwrap().is_empty(), "le premier relevé ne fait pas partir la tâche");
    // La branche distante avance de deux commits.
    let depot_head = git(&depot, &["rev-parse", "HEAD"]);
    commit(&remote, "docs.md", "Ajoute la doc des remises");
    let second = commit(&remote, "api.md", "Corrige le calcul des remises");
    poll(&mut e, |_| !seen.lock().unwrap().is_empty());
    let ctx = seen.lock().unwrap()[0].clone();
    assert!(ctx.trigger.starts_with("origin/main a avancé :"), "{}", ctx.trigger);
    assert!(
        ctx.notes.contains("Ajoute la doc des remises") && ctx.notes.contains("Corrige le calcul des remises"),
        "{}",
        ctx.notes
    );
    assert!(ctx.vars.contains(&("PATRONUS_COMMIT_APRES".into(), second.clone())));
    assert!(ctx.vars.contains(&("PATRONUS_BRANCHE".into(), "origin/main".into())));
    assert_eq!(known(&data).as_deref(), Some(second.as_str()));
    // La copie de travail n'est pas touchée ; seule la branche de suivi avance.
    assert_eq!(git(&depot, &["rev-parse", "HEAD"]), depot_head);
    assert_eq!(git(&depot, &["status", "--porcelain"]), "");
    assert_eq!(git(&depot, &["rev-parse", "origin/main"]), second);
    // Plus rien de neuf : pas de nouveau départ.
    poll(&mut e, |e| e.runs.len() == 1);
    std::thread::sleep(Duration::from_millis(300));
    e.tick(now());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// Une branche qui a avancé pendant que Patronus était fermé fait partir la tâche à l'ouverture.
#[test]
fn moved_while_closed_triggers_once_at_opening() {
    let dir = temp("ferme");
    let (remote, depot) = repos(&dir);
    let data = dir.join("donnees");
    let (exec, seen) = recorder();
    let mut e = engine(&data, exec.clone());
    e.save_task(Some("doc"), &branch_task(&depot, "origin"), now()).unwrap();
    poll(&mut e, |_| known(&data).is_some());
    drop(e);
    commit(&remote, "notes.md", "Commit pendant la fermeture");
    let mut e = engine(&data, exec);
    poll(&mut e, |_| !seen.lock().unwrap().is_empty());
    assert!(seen.lock().unwrap()[0].notes.contains("Commit pendant la fermeture"));
}

/// Un distant inconnu est signalé une seule fois, quel que soit le nombre de relevés.
#[test]
fn unknown_remote_is_reported_once() {
    let dir = temp("inconnu");
    let (_, depot) = repos(&dir);
    let (exec, seen) = recorder();
    let mut e = engine(&dir.join("donnees"), exec);
    e.save_task(Some("doc"), &branch_task(&depot, "inexistant"), now()).unwrap();
    for _ in 0..2 {
        poll(&mut e, |e| e.problems.iter().any(|p| p.contains("branche distante inexistant/main")));
    }
    assert_eq!(e.problems.iter().filter(|p| p.contains("branche distante")).count(), 1, "{:?}", e.problems);
    assert!(seen.lock().unwrap().is_empty());
}
