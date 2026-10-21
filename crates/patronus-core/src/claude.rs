//! Action « session Claude non interactive » : `claude -p` dans un worktree dédié (ou le dépôt),
//! outils limités, budget et nombre de tours bornés. La sortie `stream-json` est gardée
//! (`sortie.jsonl`), résumée dans le journal, et le diff produit est enregistré (`diff.patch`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

use crate::exec::{Context, Outcome, log, run_process_with, work_dir};
use crate::run::{Cost, Status, read_lossy};
use crate::task::Task;

/// Outils permis quand la tâche n'en liste aucun : lecture seule.
pub const READ_ONLY_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// Jamais permis, quelle que soit la tâche : ce qui part vers l'extérieur passe par un clic.
pub const FORBIDDEN_TOOLS: [&str; 6] = [
    "Bash(git push:*)",
    "Bash(git merge:*)",
    "Bash(gh pr merge:*)",
    "Bash(gh release:*)",
    "Bash(npm publish:*)",
    "Bash(cargo publish:*)",
];

/// L'exécutable de Claude Code : `PATRONUS_CLAUDE` s'il est défini (essais), sinon `claude`.
pub fn claude_exe() -> PathBuf {
    std::env::var_os("PATRONUS_CLAUDE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("claude"))
}

/// La consigne envoyée : celle de la tâche, complétée par ce qui l'a déclenchée.
pub fn prompt(task: &Task, ctx: &Context) -> String {
    let consigne = task.action.claude.as_ref().map(|c| c.consigne.trim()).unwrap_or_default();
    let mut p = consigne.to_string();
    if !ctx.files.is_empty() {
        p.push_str("\n\nFichiers modifiés qui ont déclenché cette tâche :\n");
        for f in &ctx.files {
            p.push_str(&format!("- {}\n", f.display()));
        }
    }
    p
}

/// Arguments de `claude` pour une tâche.
pub fn args(task: &Task, ctx: &Context) -> Vec<String> {
    let c = task.action.claude.clone().unwrap_or_default();
    let mut a =
        vec!["-p".to_string(), prompt(task, ctx), "--output-format".into(), "stream-json".into(), "--verbose".into()];
    a.push("--allowedTools".into());
    if c.outils.is_empty() {
        a.extend(READ_ONLY_TOOLS.iter().map(|s| s.to_string()));
    } else {
        a.extend(c.outils.iter().cloned());
    }
    a.push("--disallowedTools".into());
    a.extend(FORBIDDEN_TOOLS.iter().map(|s| s.to_string()));
    if let Some(b) = c.budget_usd {
        a.push("--max-budget-usd".into());
        a.push(format!("{b}"));
    }
    if let Some(m) = c.mode_permission.filter(|m| !m.trim().is_empty()) {
        a.push("--permission-mode".into());
        a.push(m);
    }
    a
}

/// Ce qu'on retient du flux `stream-json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stream {
    pub assistant_messages: u32,
    pub session_id: Option<String>,
    pub result: Option<String>,
    pub subtype: Option<String>,
    pub is_error: bool,
    pub cost: Option<Cost>,
    /// Coût le plus élevé annoncé jusqu'ici (`total_cost_usd` de n'importe quel événement).
    pub spent: f64,
    /// Lignes lisibles pour le journal : textes de Claude et outils appelés.
    pub transcript: Vec<String>,
}

fn short(v: &Value, max: usize) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let s = s.replace('\n', " ");
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s }
}

pub fn parse_stream(text: &str) -> Stream {
    let mut s = Stream::default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(c) = v["total_cost_usd"].as_f64() {
            s.spent = s.spent.max(c);
        }
        if let Some(id) = v["session_id"].as_str() {
            s.session_id.get_or_insert_with(|| id.to_string());
        }
        match v["type"].as_str() {
            Some("assistant") => {
                s.assistant_messages += 1;
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            s.transcript.push(format!("Claude : {}", block["text"].as_str().unwrap_or("").trim()))
                        }
                        Some("tool_use") => s.transcript.push(format!(
                            "→ {} {}",
                            block["name"].as_str().unwrap_or("?"),
                            short(&block["input"], 160)
                        )),
                        _ => {}
                    }
                }
            }
            Some("result") => {
                s.result = v["result"].as_str().map(str::to_string);
                s.subtype = v["subtype"].as_str().map(str::to_string);
                s.is_error = v["is_error"].as_bool().unwrap_or(false);
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                s.cost = Some(Cost {
                    tokens_entree: n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"),
                    tokens_sortie: n("output_tokens"),
                    usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
                });
            }
            _ => {}
        }
    }
    s
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("git introuvable : {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Crée le worktree de l'exécution, détaché sur la branche demandée. Rend son dossier et le commit
/// de départ. Le dépôt de travail de l'utilisateur n'est pas touché (ni branche créée, ni fichier).
pub fn create_worktree(repo: &Path, branch: &str, dest: &Path) -> Result<String, String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    git(repo, &["worktree", "add", "--detach", &dest.to_string_lossy(), branch])?;
    Ok(git(dest, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// Le diff de tout ce que la session a changé depuis `base` (commits et fichiers, nouveaux compris).
pub fn worktree_diff(wt: &Path, base: &str) -> Result<String, String> {
    git(wt, &["add", "-A"])?;
    git(wt, &["diff", "--cached", "--binary", base])
}

pub fn remove_worktree(repo: &Path, wt: &Path) -> Result<(), String> {
    git(repo, &["worktree", "remove", "--force", &wt.to_string_lossy()]).map(|_| ())
}

pub fn run_claude(
    _task_id: &str,
    task: &Task,
    ctx: &Context,
    journal: &Path,
    stop: &AtomicBool,
    data_dir: &Path,
    run_id: &str,
) -> Outcome {
    let Some(c) = &task.action.claude else { return Outcome::impossible("La tâche n'a pas de session Claude.") };
    let run_dir = journal.parent().map(Path::to_path_buf).unwrap_or_default();
    log(journal, &format!("[patronus] {} — {}", task.nom, ctx.trigger));
    // Dossier de travail : un worktree dédié si demandé, sinon le dépôt.
    let mut worktree = None;
    let dir = match (&task.action.worktree, &task.depot) {
        (Some(wt), Some(repo)) => {
            let dest = data_dir.join("worktrees").join(run_id);
            match create_worktree(repo, &wt.branche, &dest) {
                Ok(base) => {
                    log(
                        journal,
                        &format!(
                            "[patronus] worktree {} sur {} ({})",
                            dest.display(),
                            wt.branche,
                            &base[..base.len().min(10)]
                        ),
                    );
                    worktree = Some((repo.clone(), dest.clone(), base));
                    dest
                }
                Err(e) => {
                    let msg = format!("Worktree impossible ({} sur {}) : {e}", repo.display(), wt.branche);
                    log(journal, &format!("[patronus] {msg}"));
                    return Outcome::impossible(msg);
                }
            }
        }
        _ => work_dir(task, None),
    };
    if !dir.is_dir() {
        let msg = format!("Dossier de travail introuvable : {}", dir.display());
        log(journal, &format!("[patronus] {msg}"));
        return Outcome::impossible(msg);
    }
    let args = args(task, ctx);
    log(journal, &format!("[patronus] claude {}", args.iter().skip(2).cloned().collect::<Vec<_>>().join(" ")));
    log(journal, &format!("[patronus] consigne :\n{}\n", prompt(task, ctx)));
    let mut cmd = Command::new(claude_exe());
    cmd.args(&args).current_dir(&dir);
    let stream_path = run_dir.join("sortie.jsonl");
    let max = Duration::from_secs(task.garde_fous.duree_max_min as u64 * 60);
    let turns_max = c.tours_max;
    let budget = c.budget_usd;
    // Pendant l'exécution : tours et coût annoncés dans le flux. Claude Code applique aussi le budget
    // (`--max-budget-usd`) ; ce contrôle est le filet si un événement le dépasse quand même.
    let mut check = || {
        let s = parse_stream(&read_lossy(&stream_path));
        match (turns_max, budget) {
            (_, Some(b)) if s.spent > b => {
                Some((Status::BudgetAtteint, format!("budget de {b:.2} $ dépassé ({:.2} $)", s.spent)))
            }
            (Some(t), _) if s.assistant_messages > t => {
                Some((Status::Arretee, format!("nombre de tours maximal ({t}) atteint")))
            }
            _ => None,
        }
    };
    let (mut status, code) = run_process_with(cmd, journal, Some(&stream_path), stop, max, &mut check);
    let stream = parse_stream(&read_lossy(&stream_path));
    for line in &stream.transcript {
        log(journal, line);
    }
    let mut summary = stream.result.clone().unwrap_or_default();
    if status == Status::Reussie && stream.is_error {
        status = Status::Echouee;
    }
    if status == Status::BudgetAtteint || stream.subtype.as_deref().is_some_and(|s| s.contains("budget")) {
        status = Status::BudgetAtteint;
        summary = format!("Arrêtée : budget de {:.2} $ atteint.\n{summary}", c.budget_usd.unwrap_or(0.0));
    }
    let turns_hit = turns_max.is_some_and(|t| stream.assistant_messages > t)
        && status == Status::Arretee
        && !stop.load(std::sync::atomic::Ordering::Relaxed);
    if turns_hit {
        summary = format!("Arrêtée : nombre de tours maximal ({}) atteint.\n{summary}", turns_max.unwrap_or(0));
    }
    match status {
        Status::Impossible => {
            summary = format!("Claude Code n'a pas pu être lancé ({}) : voir le journal.", claude_exe().display())
        }
        Status::DureeDepassee => {
            summary = format!("Arrêtée : durée maximale ({} min) dépassée.\n{summary}", task.garde_fous.duree_max_min)
        }
        Status::Arretee if !turns_hit => summary = format!("Arrêtée à la main.\n{summary}"),
        Status::Echouee if summary.is_empty() => {
            summary = format!("Échec (code {}).", code.map_or("inconnu".into(), |c| c.to_string()));
        }
        _ => {}
    }
    // Le worktree est gardé s'il a produit quelque chose, avec son diff ; sinon il est supprimé.
    let mut kept = None;
    if let Some((repo, wt, base)) = worktree {
        match worktree_diff(&wt, &base) {
            Ok(diff) if !diff.trim().is_empty() => {
                if let Err(e) = crate::fsutil::write_atomic(&run_dir.join("diff.patch"), diff.as_bytes()) {
                    log(journal, &format!("[patronus] diff non enregistré : {e}"));
                }
                let files = diff.lines().filter(|l| l.starts_with("diff --git")).count();
                summary.push_str(&format!("\n\nDiff proposé : {files} fichier(s), worktree gardé ({}).", wt.display()));
                kept = Some(wt);
            }
            Ok(_) => {
                if let Err(e) = remove_worktree(&repo, &wt) {
                    log(journal, &format!("[patronus] worktree non supprimé : {e}"));
                    kept = Some(wt);
                }
            }
            Err(e) => {
                log(journal, &format!("[patronus] diff impossible, worktree gardé : {e}"));
                kept = Some(wt);
            }
        }
    }
    let mut out =
        Outcome { status, code, summary: summary.trim().to_string(), cost: stream.cost, worktree: kept, session: None };
    if let Some(id) = stream.session_id {
        log(journal, &format!("[patronus] session Claude : {id}"));
        out.session = Some(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{ClaudeAction, Worktree};

    fn claude_task() -> Task {
        let mut t = Task::new_script("Revue contoso");
        t.action.script = None;
        t.action.claude =
            Some(ClaudeAction { consigne: "Relis le code.".into(), budget_usd: Some(0.5), ..Default::default() });
        t
    }

    #[test]
    fn read_only_by_default_and_never_push() {
        let a = args(&claude_task(), &Context::default());
        let allowed = a.iter().position(|x| x == "--allowedTools").unwrap();
        assert_eq!(&a[allowed + 1..allowed + 4], ["Read", "Grep", "Glob"]);
        assert!(a.iter().any(|x| x == "Bash(git push:*)"));
        assert!(a.windows(2).any(|w| w[0] == "--max-budget-usd" && w[1] == "0.5"));
        assert!(!a.iter().any(|x| x == "--permission-mode"));
    }

    #[test]
    fn prompt_lists_the_changed_files() {
        let ctx = Context { trigger: "x".into(), files: vec![PathBuf::from("docs/guide.md")] };
        let p = prompt(&claude_task(), &ctx);
        assert!(p.starts_with("Relis le code."));
        assert!(p.contains("- docs/guide.md"));
    }

    #[test]
    fn stream_is_summarised() {
        let text = r#"{"type":"system","subtype":"init","session_id":"abc"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Je regarde."},{"type":"tool_use","name":"Read","input":{"file_path":"Cargo.toml"}}]}}
pas du json
{"type":"assistant","message":{"content":[{"type":"text","text":"Fini."}]}}
{"type":"result","subtype":"success","is_error":false,"result":"Deux mises à jour mineures.","total_cost_usd":0.0421,"usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":25}}"#;
        let s = parse_stream(text);
        assert_eq!(s.assistant_messages, 2);
        assert_eq!(s.session_id.as_deref(), Some("abc"));
        assert_eq!(s.result.as_deref(), Some("Deux mises à jour mineures."));
        assert_eq!(s.cost, Some(Cost { tokens_entree: 100, tokens_sortie: 25, usd: 0.0421 }));
        assert_eq!(s.transcript[1], r#"→ Read {"file_path":"Cargo.toml"}"#);
    }

    #[test]
    fn worktree_needs_a_repo() {
        let mut t = claude_task();
        t.action.worktree = Some(Worktree { branche: "main".into() });
        assert!(t.validate().join(" ").contains("worktree demande un dépôt"));
    }
}
