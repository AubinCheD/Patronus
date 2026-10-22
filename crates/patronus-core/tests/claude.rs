//! L'action « session Claude » de bout en bout, avec le faux exécutable (`faux-claude`) : aucun vrai
//! `claude -p` n'est lancé.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use patronus_core::claude::run_claude;
use patronus_core::exec::{Context, Outcome};
use patronus_core::run::Status;
use patronus_core::task::{ClaudeAction, Task, Worktree};

fn setup(name: &str) -> PathBuf {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // Même valeur pour tous les tests, posée une seule fois.
    ONCE.call_once(|| unsafe { std::env::set_var("PATRONUS_CLAUDE", env!("CARGO_BIN_EXE_faux-claude")) });
    let dir = std::env::temp_dir().join(format!("patronus-claude-{name}-{}", std::process::id()));
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

/// Un dépôt fictif `contoso-api`, branche `main`, un commit.
fn repo(dir: &Path) -> PathBuf {
    let r = dir.join("contoso-api");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    std::fs::write(r.join("README.md"), "contoso\n").unwrap();
    git(&r, &["add", "."]);
    git(&r, &["commit", "-q", "-m", "Premier commit"]);
    r
}

fn task(consigne: &str, depot: &Path, worktree: bool) -> Task {
    let mut t = Task::new_script("Revue contoso");
    t.action.script = None;
    t.action.claude = Some(ClaudeAction {
        consigne: consigne.into(),
        budget_usd: Some(0.5),
        tours_max: Some(5),
        ..Default::default()
    });
    t.action.worktree = worktree.then(|| Worktree::on("main"));
    t.depot = Some(depot.to_path_buf());
    assert!(t.validate().is_empty(), "{:?}", t.validate());
    t
}

fn run(dir: &Path, t: &Task, id: &str) -> (Outcome, String) {
    let run_dir = dir.join("donnees").join("executions").join(id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let journal = run_dir.join("journal.txt");
    let ctx = Context { trigger: "à la main".into(), files: vec![], ..Default::default() };
    let out = run_claude("revue", t, &ctx, &journal, &AtomicBool::new(false), &dir.join("donnees"), id);
    (out, std::fs::read_to_string(&journal).unwrap_or_default())
}

#[test]
fn result_cost_and_session_reach_the_outcome() {
    let dir = setup("ok");
    let (out, journal) = run(&dir, &task("Relis le code.", &dir, false), "r1");
    assert_eq!(out.status, Status::Reussie, "{journal}");
    assert!(out.summary.contains("Résumé du faux"), "{}", out.summary);
    assert!(out.summary.contains("Read Grep Glob"), "lecture seule par défaut : {}", out.summary);
    let cost = out.cost.unwrap();
    assert_eq!((cost.tokens_entree, cost.tokens_sortie), (1000, 50));
    assert!((cost.usd - 0.005).abs() < 1e-9, "{}", cost.usd);
    assert!(!cost.estime, "coût exact de Claude Code");
    assert_eq!(out.session.as_deref(), Some("faux-123"));
    assert!(journal.contains("Claude : Étape 1.") && journal.contains("→ Read"), "{journal}");
    assert!(journal.contains("--max-budget-usd 0.5") && journal.contains("--max-turns 5"), "{journal}");
}

#[test]
fn worktree_with_changes_is_kept_with_its_diff() {
    let dir = setup("wt");
    let r = repo(&dir);
    let (out, journal) = run(&dir, &task("Propose une doc. [ecrire]", &r, true), "r2");
    assert_eq!(out.status, Status::Reussie, "{journal}");
    let wt = out.worktree.clone().expect("worktree gardé");
    assert!(wt.join("PATRONUS-ESSAI.md").exists());
    assert!(!r.join("PATRONUS-ESSAI.md").exists(), "la copie de travail n'est pas touchée");
    assert_eq!(git(&r, &["status", "--porcelain"]), "", "dépôt de l'utilisateur propre");
    let diff = std::fs::read_to_string(dir.join("donnees/executions/r2/diff.patch")).unwrap();
    assert!(diff.contains("PATRONUS-ESSAI.md") && diff.contains("+Proposition de la session."));
    assert!(out.summary.contains("Diff proposé : 1 fichier"), "{}", out.summary);
}

#[test]
fn worktree_without_changes_is_removed() {
    let dir = setup("wt-vide");
    let r = repo(&dir);
    let (out, journal) = run(&dir, &task("Regarde seulement.", &r, true), "r3");
    assert_eq!(out.status, Status::Reussie, "{journal}");
    assert!(out.worktree.is_none());
    assert!(!dir.join("donnees/worktrees/r3").exists());
    assert_eq!(git(&r, &["worktree", "list"]).lines().count(), 1);
    assert!(!dir.join("donnees/executions/r3/diff.patch").exists());
}

/// Claude Code arrête lui-même la session au nombre de tours (`--max-turns`).
#[test]
fn max_turns_from_claude_code() {
    let dir = setup("tours");
    let mut t = task("Longue. [tours:20]", &dir, false);
    t.action.claude.as_mut().unwrap().tours_max = Some(2);
    let (out, journal) = run(&dir, &t, "r4");
    assert_eq!(out.status, Status::ToursAtteints, "{journal}");
    assert!(out.summary.contains("Arrêtée par Claude Code : nombre de tours maximal (2)"), "{}", out.summary);
    assert_eq!(journal.matches("Claude : Étape").count(), 2, "un message par réponse, pas par bloc : {journal}");
}

/// Claude Code n'applique pas le budget : Patronus l'estime d'après l'usage de chaque message et
/// arrête la session, sans attendre sa fin ; le coût estimé est gardé.
#[test]
fn estimated_budget_stops_the_session() {
    let dir = setup("budget");
    let start = std::time::Instant::now();
    let (out, journal) = run(&dir, &task("Chère. [tours:20][lent][gros][ignorer-budget]", &dir, false), "r5");
    assert_eq!(out.status, Status::BudgetAtteint, "{journal}");
    assert!(out.summary.contains("Arrêtée par Patronus : budget de 0.50 $"), "{}", out.summary);
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "arrêtée avant la fin (6 s)");
    let cost = out.cost.expect("coût estimé");
    assert!(cost.estime && cost.usd > 0.5, "{cost:?}");
    assert!(journal.contains("d'après l'usage annoncé"), "{journal}");
}

/// Claude Code arrête lui-même la session au budget (`--max-budget-usd`).
#[test]
fn budget_from_claude_code() {
    let dir = setup("budget-cli");
    let (out, journal) = run(&dir, &task("[tours:20][gros]", &dir, false), "r6");
    assert_eq!(out.status, Status::BudgetAtteint, "{journal}");
    assert!(out.summary.contains("Arrêtée par Claude Code : budget de 0.50 $"), "{}", out.summary);
    assert!(!out.cost.unwrap().estime);
}

/// Une session qui se termine sans résultat n'est pas « réussie ».
#[test]
fn no_result_is_a_failure() {
    let dir = setup("sans-resultat");
    let (out, journal) = run(&dir, &task("[sans-resultat]", &dir, false), "r12");
    assert_eq!(out.status, Status::Echouee, "{journal}");
    assert!(out.summary.contains("sans rendre de résultat"), "{}", out.summary);
    assert!(out.cost.is_some_and(|c| c.estime), "coût estimé malgré tout");
}

/// Les appels d'outils refusés sont listés dans le résumé.
#[test]
fn denials_are_listed() {
    let dir = setup("refus");
    let (out, _) = run(&dir, &task("[refus]", &dir, false), "r13");
    assert_eq!(out.status, Status::Reussie);
    assert!(out.summary.contains("Appels refusés (1) : Bash"), "{}", out.summary);
    assert!(out.summary.contains("git -C . push"), "{}", out.summary);
}

/// Arrêtée à la main avant son résultat : le coût estimé est gardé.
#[test]
fn stopped_session_keeps_its_estimated_cost() {
    let dir = setup("arret");
    let t = task("[tours:20][lent]", &dir, false);
    let run_dir = dir.join("donnees").join("executions").join("r14");
    std::fs::create_dir_all(&run_dir).unwrap();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (s2, stream) = (stop.clone(), run_dir.join("sortie.jsonl"));
    // Arrêtée dès son premier message, quel que soit le temps que met le faux exécutable à démarrer.
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        while !std::fs::read_to_string(&stream).is_ok_and(|s| s.contains("\"assistant\""))
            && start.elapsed() < std::time::Duration::from_secs(15)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        s2.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let ctx = Context { trigger: "à la main".into(), files: vec![], ..Default::default() };
    let out = run_claude("revue", &t, &ctx, &run_dir.join("journal.txt"), &stop, &dir.join("donnees"), "r14");
    assert_eq!(out.status, Status::Arretee, "{}", out.summary);
    let cost = out.cost.expect("coût estimé");
    assert!(cost.estime && cost.usd > 0.0, "{cost:?}");
}

#[test]
fn failure_and_missing_branch_say_why() {
    let dir = setup("echec");
    let (out, journal) = run(&dir, &task("[erreur]", &dir, false), "r7");
    assert_eq!(out.status, Status::Echouee);
    assert!(journal.contains("erreur simulée"), "{journal}");
    let r = repo(&dir);
    let mut t = task("x", &r, true);
    t.action.worktree = Some(Worktree::on("inexistante"));
    let (out, _) = run(&dir, &t, "r8");
    assert_eq!(out.status, Status::Impossible);
    assert!(out.summary.contains("Worktree impossible"), "{}", out.summary);
}

fn run_ctx(dir: &Path, t: &Task, id: &str, ctx: &Context, exe: Option<&Path>) -> (Outcome, String) {
    let run_dir = dir.join("donnees").join("executions").join(id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let journal = run_dir.join("journal.txt");
    let stop = AtomicBool::new(false);
    let out = match exe {
        Some(exe) => patronus_core::claude::run_claude_with(exe, t, ctx, &journal, &stop, &dir.join("donnees"), id),
        None => run_claude("revue", t, ctx, &journal, &stop, &dir.join("donnees"), id),
    };
    (out, std::fs::read_to_string(&journal).unwrap_or_default())
}

/// 800 fichiers modifiés : la consigne en cite 50, la liste complète est dans `fichiers.txt`.
/// (En argument, la ligne de commande dépassait la limite de Windows.)
#[test]
fn many_changed_files_are_capped() {
    let dir = setup("fichiers");
    let files: Vec<PathBuf> =
        (0..800).map(|i| PathBuf::from(format!(r"C:\dev\northwind\docs\page-{i:03}.md"))).collect();
    let ctx = Context { trigger: "800 fichiers modifiés".into(), files, ..Default::default() };
    let (out, journal) = run_ctx(&dir, &task("Relis les pages modifiées.", &dir, false), "r9", &ctx, None);
    assert_eq!(out.status, Status::Reussie, "{journal}");
    let run_dir = dir.join("donnees/executions/r9");
    let consigne = std::fs::read_to_string(run_dir.join("consigne.txt")).unwrap();
    assert!(consigne.contains("page-049.md") && !consigne.contains("page-050.md"), "{consigne}");
    assert!(consigne.contains("… et 750 autres (800 au total"), "{consigne}");
    assert_eq!(std::fs::read_to_string(run_dir.join("fichiers.txt")).unwrap().lines().count(), 800);
}

/// Une consigne qui commence par un tiret n'est pas prise pour une option.
#[test]
fn prompt_starting_with_a_dash() {
    let dir = setup("tiret");
    let ctx = Context { trigger: "à la main".into(), files: vec![], ..Default::default() };
    let (out, journal) = run_ctx(&dir, &task("-x : ceci n'est pas une option", &dir, false), "r10", &ctx, None);
    assert_eq!(out.status, Status::Reussie, "{journal}");
    assert!(out.summary.contains("consigne : -x : ceci n'est pas une option"), "{}", out.summary);
}

/// Installation de Claude Code par npm : `claude.cmd`. Consigne sur plusieurs lignes et motifs
/// d'outils (parenthèses, deux-points, astérisques) passent.
#[test]
fn cmd_shim_runs_with_a_multiline_prompt() {
    let dir = setup("cmd");
    let shim = dir.join("claude.cmd");
    std::fs::write(&shim, format!("@\"{}\" %*\r\n", env!("CARGO_BIN_EXE_faux-claude"))).unwrap();
    let mut t = task("Première ligne.\nDeuxième ligne.", &dir, false);
    t.action.claude.as_mut().unwrap().outils = vec!["Read".into(), "Bash(npm outdated)".into()];
    let ctx =
        Context { trigger: "à la main".into(), files: vec![PathBuf::from("docs/guide.md")], ..Default::default() };
    let (out, journal) = run_ctx(&dir, &t, "r11", &ctx, Some(&shim));
    assert_eq!(out.status, Status::Reussie, "{journal}");
    assert!(out.summary.contains("outils Read Bash(npm outdated)"), "{}", out.summary);
    assert!(out.summary.contains("consigne : Première ligne."), "{}", out.summary);
}

/// Ce que le processus `claude` reçoit vraiment : mode restreint, outils, interdits, et le garde-fou
/// de push dans l'environnement (réécriture des adresses, crochet `pre-push` de Patronus).
#[test]
fn session_receives_isolation_args_and_push_guard() {
    let dir = setup("trace");
    let r = repo(&dir);
    let (out, journal) = run(&dir, &task("Relis le code. [trace]", &r, true), "r12");
    assert_eq!(out.status, Status::Reussie, "{journal}");
    let s = &out.summary;
    for arg in ["--restricted", "--strict-mcp-config", "--tools Read,Grep,Glob", "Bash(git push:*)", "--max-turns 5"] {
        assert!(s.contains(arg), "argument {arg} : {s}");
    }
    assert!(s.contains("git : url.patronus-push-interdit:.pushInsteadOf=\n"), "{s}");
    let hooks = dir.join("donnees").join("garde-fous").join("crochets");
    let expected = format!("git : core.hooksPath={}", hooks.to_string_lossy().replace('\\', "/"));
    assert!(s.contains(&expected), "{s}");
    assert!(std::fs::read_to_string(hooks.join("pre-push")).unwrap().contains("exit 1"));
}
