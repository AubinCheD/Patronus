//! Action « session Claude non interactive » : `claude -p` dans un worktree dédié (ou le dépôt),
//! outils limités, budget et nombre de tours bornés. La sortie `stream-json` est gardée
//! (`sortie.jsonl`), résumée dans le journal, et le diff produit est enregistré (`diff.patch`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::exec::{
    Context, MAX_FILES_IN_PROMPT, Outcome, capped_list, log, run_process_with, work_dir, write_file_list,
};
use crate::pricing::{Usage, cost, price_for};
use crate::run::{Cost, Status};
use crate::task::{Task, tool_name};

/// Outils permis quand la tâche n'en liste aucun : lecture seule.
pub const DEFAULT_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// Jamais permis, quelle que soit la tâche : ce qui part vers l'extérieur passe par un clic. Une règle
/// ne vaut que pour le début de la commande (`git -C . push` échappe à `Bash(git push:*)`) : le push
/// est en plus rendu impossible au niveau de git (`git_guard_env`).
pub const FORBIDDEN_TOOLS: [&str; 22] = [
    "Bash(git push:*)",
    "Bash(git -C * push:*)",
    "Bash(git -c * push:*)",
    "Bash(git merge:*)",
    "Bash(git -C * merge:*)",
    "Bash(gh pr create:*)",
    "Bash(gh pr comment:*)",
    "Bash(gh pr merge:*)",
    "Bash(gh pr review:*)",
    "Bash(gh pr close:*)",
    "Bash(gh issue create:*)",
    "Bash(gh issue comment:*)",
    "Bash(gh issue edit:*)",
    "Bash(gh issue close:*)",
    "Bash(gh api:*)",
    "Bash(gh workflow run:*)",
    "Bash(gh release:*)",
    "Bash(npm publish:*)",
    "Bash(cargo publish:*)",
    "Bash(dotnet nuget push:*)",
    "Bash(docker push:*)",
    "Bash(az repos pr:*)",
];

/// Variables d'environnement de la session, ajoutées après la configuration déjà passée par
/// `GIT_CONFIG_COUNT` :
/// - toute adresse de push est réécrite vers un protocole qui n'existe pas (`url.<x>.pushInsteadOf`
///   vide) ;
/// - les crochets sont ceux de `hooks` (`core.hooksPath`), dont un `pre-push` qui refuse tout push :
///   il couvre un distant à `pushurl` explicite, que git ne réécrit pas ;
/// - git ne demande jamais d'identifiants.
///
/// Un `git push --no-verify` vers un `pushurl` explicite y échapperait encore : la première barrière
/// reste la liste des outils interdits de Claude Code.
pub fn git_guard_env(existing_count: Option<&str>, hooks: &Path) -> Vec<(String, String)> {
    let n: usize = existing_count.and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    vec![
        ("GIT_CONFIG_COUNT".into(), (n + 2).to_string()),
        (format!("GIT_CONFIG_KEY_{n}"), "url.patronus-push-interdit:.pushInsteadOf".into()),
        (format!("GIT_CONFIG_VALUE_{n}"), String::new()),
        (format!("GIT_CONFIG_KEY_{}", n + 1), "core.hooksPath".into()),
        (format!("GIT_CONFIG_VALUE_{}", n + 1), hooks.to_string_lossy().replace('\\', "/")),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GCM_INTERACTIVE".into(), "never".into()),
    ]
}

/// Crochet `pre-push` des sessions (fins de ligne LF : git le lance par `sh`).
const PRE_PUSH: &str = "#!/bin/sh\n\
    echo \"Patronus : pas de push pendant une session ; relisez le résultat puis poussez vous-même.\" >&2\n\
    exit 1\n";

/// Écrit les crochets des sessions dans `<données>\garde-fous\crochets` et rend ce dossier. Ceux
/// du dépôt sont ignorés pendant la session : rien n'y est commité pour l'utilisateur, dont les
/// crochets jouent quand il commite le diff appliqué.
pub fn guard_hooks(data_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("garde-fous").join("crochets");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("pre-push"), PRE_PUSH)?;
    Ok(dir)
}

/// L'exécutable de Claude Code : `PATRONUS_CLAUDE` s'il est défini (essais), sinon le premier
/// `claude.exe` ou `claude.cmd` du `PATH` (installation native ou par npm), sinon `claude`.
pub fn claude_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("PATRONUS_CLAUDE") {
        return PathBuf::from(p);
    }
    find_in_path(std::env::var_os("PATH").as_deref()).unwrap_or_else(|| PathBuf::from("claude"))
}

/// Comme Windows : dossier par dossier dans l'ordre du `PATH`, `.exe` avant `.cmd` dans un même dossier.
pub fn find_in_path(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs.iter().flat_map(|d| [d.join("claude.exe"), d.join("claude.cmd")]).find(|c| c.is_file())
}

/// La consigne envoyée : celle de la tâche, complétée par ce qui l'a déclenchée.
pub fn prompt(task: &Task, ctx: &Context) -> String {
    let consigne = task.action.claude.as_ref().map(|c| c.consigne.trim()).unwrap_or_default();
    let mut p = consigne.to_string();
    if !ctx.files.is_empty() {
        p.push_str("\n\nFichiers modifiés qui ont déclenché cette tâche :\n");
        for line in capped_list(&ctx.files, MAX_FILES_IN_PROMPT).lines() {
            p.push_str(&format!("- {line}\n"));
        }
    }
    if !ctx.notes.trim().is_empty() {
        p.push_str("\n\n");
        p.push_str(ctx.notes.trim());
        p.push('\n');
    }
    p
}

/// Arguments de `claude` pour une tâche. La consigne n'y figure pas : elle passe par l'entrée
/// standard (ni limite de longueur, ni retour à la ligne refusé vers un `claude.cmd`, ni consigne
/// prise pour une option).
///
/// Mode restreint : les réglages de l'utilisateur, du dépôt et locaux (qui élargiraient la liste des
/// outils) sont ignorés, les outils de fichiers confinés au dossier de travail, `bypassPermissions`
/// refusé ; `--tools` ne laisse exister que les outils intégrés de la tâche, et `--strict-mcp-config`
/// écarte les serveurs MCP configurés ailleurs.
pub fn args(task: &Task) -> Vec<String> {
    let c = task.action.claude.clone().unwrap_or_default();
    let allowed: Vec<String> =
        if c.outils.is_empty() { DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect() } else { c.outils.clone() };
    let mut builtin: Vec<&str> = Vec::new();
    for t in &allowed {
        let name = tool_name(t);
        if !name.starts_with("mcp__") && !builtin.contains(&name) {
            builtin.push(name);
        }
    }
    let mut a = vec!["-p".to_string(), "--output-format".into(), "stream-json".into(), "--verbose".into()];
    a.extend(["--restricted".to_string(), "--strict-mcp-config".into(), "--tools".into(), builtin.join(",")]);
    a.push("--allowedTools".into());
    a.extend(allowed);
    a.push("--disallowedTools".into());
    a.extend(FORBIDDEN_TOOLS.iter().map(|s| s.to_string()));
    if let Some(b) = c.budget_usd {
        a.push("--max-budget-usd".into());
        a.push(format!("{b}"));
    }
    if let Some(t) = c.tours_max {
        a.push("--max-turns".into());
        a.push(t.to_string());
    }
    if let Some(m) = c.mode_permission.map(|m| m.trim().to_string()).filter(|m| !m.is_empty() && m != "default") {
        a.push("--permission-mode".into());
        a.push(m);
    }
    a
}

/// Un message de Claude, dédoublonné : Claude Code émet un événement `assistant` par bloc de contenu,
/// tous avec le même `message.id`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Message {
    model: String,
    usage: Usage,
    /// Texte visible du message (réponses et arguments d'outils), en caractères : les tokens de sortie
    /// annoncés pendant la session sont provisoires, on estime d'après ce texte (minorant : la
    /// réflexion n'est pas visible).
    visible_chars: usize,
}

/// Appel d'outil refusé (`permission_denials` du résultat).
#[derive(Clone, Debug, PartialEq)]
pub struct Denial {
    pub tool: String,
    pub input: String,
}

/// Ce qu'on retient du flux `stream-json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stream {
    messages: HashMap<String, Message>,
    /// Ordre d'arrivée des messages.
    order: Vec<String>,
    pub session_id: Option<String>,
    /// Fin de session (`result`) reçue.
    pub finished: bool,
    pub result: Option<String>,
    pub subtype: Option<String>,
    pub is_error: bool,
    /// Erreurs annoncées par un résultat en échec (`errors`).
    pub errors: Vec<String>,
    pub denials: Vec<Denial>,
    /// Coût exact, annoncé par Claude Code à la fin.
    pub cost: Option<Cost>,
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

impl Stream {
    /// Messages distincts de Claude : à peu près le nombre de tours.
    pub fn turns(&self) -> usize {
        self.messages.len()
    }

    /// Coût estimé d'après l'usage annoncé, ou rien si un modèle est inconnu de `pricing`.
    pub fn estimate(&self) -> Option<f64> {
        let mut total = 0.0;
        for m in self.messages.values() {
            let price = price_for(&m.model)?;
            let output = m.usage.output.max((m.visible_chars / 4) as u64);
            total += cost(price, Usage { output, ..m.usage });
        }
        Some(total)
    }

    /// Le coût à retenir : celui de Claude Code s'il est arrivé, sinon l'estimation (marquée).
    pub fn final_cost(&self) -> Option<Cost> {
        if let Some(c) = &self.cost {
            return Some(c.clone());
        }
        let usd = self.estimate()?;
        let (mut input, mut output) = (0, 0);
        for m in self.messages.values() {
            input += m.usage.input + m.usage.cache_write + m.usage.cache_read;
            output += m.usage.output.max((m.visible_chars / 4) as u64);
        }
        Some(Cost { tokens_entree: input, tokens_sortie: output, usd, estime: true })
    }

    /// Ajoute une ligne du flux.
    pub fn feed(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if let Some(id) = v["session_id"].as_str() {
            self.session_id.get_or_insert_with(|| id.to_string());
        }
        match v["type"].as_str() {
            Some("assistant") => {
                let msg = &v["message"];
                let id =
                    msg["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("sans-id-{}", self.order.len()));
                if !self.messages.contains_key(&id) {
                    self.order.push(id.clone());
                }
                let m = self.messages.entry(id).or_default();
                if let Some(model) = msg["model"].as_str() {
                    m.model = model.to_string();
                }
                m.usage = m.usage.max(Usage::from_json(&msg["usage"]));
                for block in msg["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("").trim();
                            m.visible_chars += text.chars().count();
                            self.transcript.push(format!("Claude : {text}"));
                        }
                        Some("tool_use") => {
                            m.visible_chars += block["input"].to_string().chars().count();
                            self.transcript.push(format!(
                                "→ {} {}",
                                block["name"].as_str().unwrap_or("?"),
                                short(&block["input"], 160)
                            ));
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                self.finished = true;
                self.result = v["result"].as_str().map(str::to_string);
                self.subtype = v["subtype"].as_str().map(str::to_string);
                self.is_error = v["is_error"].as_bool().unwrap_or(false);
                self.errors = v["errors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str())
                    .map(str::to_string)
                    .collect();
                self.denials = v["permission_denials"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| Denial {
                        tool: d["tool_name"].as_str().unwrap_or("?").to_string(),
                        input: short(&d["tool_input"], 120),
                    })
                    .collect();
                let u = Usage::from_json(&v["usage"]);
                self.cost = Some(Cost {
                    tokens_entree: u.input + u.cache_write + u.cache_read,
                    tokens_sortie: u.output,
                    usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
                    estime: false,
                });
            }
            _ => {}
        }
    }
}

/// Le flux en entier (tests).
pub fn parse_stream(text: &str) -> Stream {
    let mut s = Stream::default();
    for line in text.lines() {
        s.feed(line);
    }
    s
}

/// Lecture du flux au fil de l'eau : seules les lignes ajoutées depuis la dernière fois sont lues.
pub struct StreamReader {
    path: PathBuf,
    offset: u64,
    partial: String,
    pub stream: Stream,
}

impl StreamReader {
    pub fn new(path: &Path) -> StreamReader {
        StreamReader { path: path.to_path_buf(), offset: 0, partial: String::new(), stream: Stream::default() }
    }

    /// Lit ce qui a été ajouté au fichier ; une ligne incomplète attend la suite.
    pub fn poll(&mut self) -> &Stream {
        use std::io::{Read, Seek, SeekFrom};
        if let Ok(mut f) = std::fs::File::open(&self.path)
            && f.seek(SeekFrom::Start(self.offset)).is_ok()
        {
            let mut bytes = Vec::new();
            if let Ok(n) = f.read_to_end(&mut bytes) {
                self.offset += n as u64;
                self.partial.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(i) = self.partial.find('\n') {
                    let line: String = self.partial.drain(..=i).collect();
                    self.stream.feed(line.trim_end());
                }
            }
        }
        &self.stream
    }

    /// Lit tout ce qui reste, y compris une dernière ligne sans fin de ligne.
    pub fn finish(mut self) -> Stream {
        self.poll();
        let rest = std::mem::take(&mut self.partial);
        if !rest.trim().is_empty() {
            self.stream.feed(rest.trim_end());
        }
        self.stream
    }
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
    // `--end-of-options` : une branche qui commencerait par un tiret n'est pas prise pour une option.
    git(repo, &["worktree", "add", "--detach", "--end-of-options", &dest.to_string_lossy(), branch])?;
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

/// Fiche du worktree d'une exécution (`worktree.json` dans son dossier), écrite dès sa création :
/// un worktree laissé par une exécution coupée peut être rapproché de son entrée.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeInfo {
    pub chemin: PathBuf,
    pub depot: PathBuf,
    /// Commit de départ : le diff de la session se calcule depuis lui.
    pub base: String,
}

pub fn worktree_info(run_dir: &Path) -> Option<WorktreeInfo> {
    let text = std::fs::read_to_string(run_dir.join("worktree.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Supprime un worktree d'après son seul chemin : le dépôt principal est retrouvé par git. Un
/// worktree déjà effacé du disque ne laisse que des métadonnées, que `git worktree prune` nettoie
/// dans `depot` s'il est connu.
pub fn discard_worktree(wt: &Path, depot: Option<&Path>) -> Result<(), String> {
    if !wt.exists() {
        if let Some(repo) = depot {
            git(repo, &["worktree", "prune"])?;
        }
        return Ok(());
    }
    let common = git(wt, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = PathBuf::from(common.trim());
    let repo = common.parent().map(Path::to_path_buf).ok_or("dépôt principal introuvable")?;
    remove_worktree(&repo, wt)
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
    run_claude_with(&claude_exe(), task, ctx, journal, stop, data_dir, run_id)
}

/// Comme `run_claude`, avec l'exécutable donné (essais : un `claude.cmd`).
pub fn run_claude_with(
    exe: &Path,
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
    let hooks = match guard_hooks(data_dir) {
        Ok(h) => h,
        Err(e) => {
            let msg = format!("Garde-fou de push non écrit (crochet pre-push) : {e}");
            log(journal, &format!("[patronus] {msg}"));
            return Outcome::impossible(msg);
        }
    };
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
                    let info = WorktreeInfo { chemin: dest.clone(), depot: repo.clone(), base: base.clone() };
                    let _ = crate::fsutil::write_atomic(
                        &run_dir.join("worktree.json"),
                        serde_json::to_string_pretty(&info).unwrap_or_default().as_bytes(),
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
    let args = args(task);
    let consigne = prompt(task, ctx);
    log(journal, &format!("[patronus] {} {}", exe.display(), args.join(" ")));
    log(journal, &format!("[patronus] consigne :\n{consigne}\n"));
    let consigne_path = run_dir.join("consigne.txt");
    if let Err(e) = crate::fsutil::write_atomic(&consigne_path, consigne.as_bytes()) {
        return Outcome::impossible(format!("Consigne non écrite ({}) : {e}", consigne_path.display()));
    }
    if !ctx.files.is_empty() {
        write_file_list(&run_dir, &ctx.files);
    }
    let mut cmd = Command::new(exe);
    cmd.args(&args).current_dir(&dir);
    cmd.envs(git_guard_env(std::env::var("GIT_CONFIG_COUNT").ok().as_deref(), &hooks));
    cmd.envs(ctx.vars.iter().map(|(k, v)| (k, v)));
    let stream_path = run_dir.join("sortie.jsonl");
    let max = Duration::from_secs(task.garde_fous.duree_max_min as u64 * 60);
    let (turns_max, budget) = (c.tours_max, c.budget_usd);
    // Pendant l'exécution, au plus une fois par seconde : coût estimé d'après l'usage annoncé, et
    // tours. Claude Code applique lui-même `--max-budget-usd` et `--max-turns` ; ces contrôles sont
    // le second garde-fou (un modèle inconnu des tarifs n'est pas estimé).
    let mut reader = StreamReader::new(&stream_path);
    let mut last_check = Instant::now();
    let mut patronus_stop: Option<Status> = None;
    let mut check = || {
        if last_check.elapsed() < Duration::from_secs(1) {
            return None;
        }
        last_check = Instant::now();
        let s = reader.poll();
        let why = match (budget, s.estimate(), turns_max) {
            (Some(b), Some(spent), _) if spent > b => Some((
                Status::BudgetAtteint,
                format!("budget de {b:.2} $ dépassé (environ {spent:.2} $ d'après l'usage annoncé)"),
            )),
            (_, _, Some(t)) if s.turns() > 2 * t as usize + 5 => {
                Some((Status::ToursAtteints, format!("nombre de tours maximal ({t}) dépassé")))
            }
            _ => None,
        };
        if let Some((status, _)) = &why {
            patronus_stop = Some(*status);
        }
        why
    };
    let (mut status, code) =
        run_process_with(cmd, journal, Some(&stream_path), Some(&consigne_path), stop, max, &mut check);
    let stream = StreamReader::new(&stream_path).finish();
    for line in &stream.transcript {
        log(journal, line);
    }
    let mut summary = stream.result.clone().unwrap_or_default();
    match stream.subtype.as_deref() {
        // Arrêts décidés par Claude Code lui-même.
        Some("error_max_turns") => {
            status = Status::ToursAtteints;
            summary =
                format!("Arrêtée par Claude Code : nombre de tours maximal ({}) atteint.", turns_max.unwrap_or(0));
        }
        Some("error_max_budget_usd") => {
            status = Status::BudgetAtteint;
            summary = format!("Arrêtée par Claude Code : budget de {:.2} $ atteint.", budget.unwrap_or(0.0));
        }
        Some(sub) if sub.starts_with("error") => {
            status = Status::Echouee;
            summary = format!("Échec de Claude Code ({sub}). {}", stream.errors.join(" "));
        }
        _ => {}
    }
    match status {
        Status::Reussie if !stream.finished => {
            status = Status::Echouee;
            summary = "Claude Code s'est terminé sans rendre de résultat (voir le journal).".into();
        }
        Status::Reussie if stream.is_error => status = Status::Echouee,
        Status::Impossible => {
            summary = format!("Claude Code n'a pas pu être lancé ({}) : voir le journal.", exe.display())
        }
        Status::DureeDepassee => {
            summary = format!("Arrêtée : durée maximale ({} min) dépassée.\n{summary}", task.garde_fous.duree_max_min)
        }
        Status::BudgetAtteint if patronus_stop == Some(Status::BudgetAtteint) => {
            summary = format!(
                "Arrêtée par Patronus : budget de {:.2} $ dépassé d'après l'usage annoncé.\n{summary}",
                budget.unwrap_or(0.0)
            )
        }
        Status::ToursAtteints if patronus_stop == Some(Status::ToursAtteints) => {
            summary = format!("Arrêtée par Patronus : nombre de tours ({}) dépassé.\n{summary}", turns_max.unwrap_or(0))
        }
        Status::Arretee => summary = format!("Arrêtée à la main.\n{summary}"),
        Status::Echouee if summary.is_empty() => {
            summary = format!("Échec (code {}).", code.map_or("inconnu".into(), |c| c.to_string()));
        }
        _ => {}
    }
    if !stream.denials.is_empty() {
        let list: Vec<String> = stream.denials.iter().map(|d| format!("{} {}", d.tool, d.input)).collect();
        summary.push_str(&format!("\n\nAppels refusés ({}) : {}", list.len(), list.join(" ; ")));
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
    let mut out = Outcome {
        status,
        code,
        summary: summary.trim().to_string(),
        cost: stream.final_cost(),
        worktree: kept,
        session: None,
    };
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
        let a = args(&claude_task());
        let allowed = a.iter().position(|x| x == "--allowedTools").unwrap();
        assert_eq!(&a[allowed + 1..allowed + 4], ["Read", "Grep", "Glob"]);
        assert!(a.iter().any(|x| x == "Bash(git push:*)"));
        assert!(a.windows(2).any(|w| w[0] == "--max-budget-usd" && w[1] == "0.5"));
        assert!(!a.iter().any(|x| x == "--permission-mode"));
    }

    #[test]
    fn restricted_mode_and_builtin_tools_only() {
        let mut t = claude_task();
        let a = args(&t);
        assert!(a.iter().any(|x| x == "--restricted") && a.iter().any(|x| x == "--strict-mcp-config"));
        let tools = |a: &[String]| a[a.iter().position(|x| x == "--tools").unwrap() + 1].clone();
        assert_eq!(tools(&a), "Read,Grep,Glob");
        let c = t.action.claude.as_mut().unwrap();
        c.outils =
            ["Read", "Edit", "Bash(npm outdated)", "Bash(cargo test)", "mcp__contoso__lire"].map(String::from).to_vec();
        c.mode_permission = Some("default".into());
        let a = args(&t);
        assert_eq!(tools(&a), "Read,Edit,Bash", "outils intégrés seulement, sans doublon");
        assert!(!a.iter().any(|x| x == "--permission-mode"), "le mode par défaut n'est pas passé");
        t.action.claude.as_mut().unwrap().mode_permission = Some("plan".into());
        assert!(args(&t).windows(2).any(|w| w[0] == "--permission-mode" && w[1] == "plan"));
    }

    #[test]
    fn push_guard_appends_to_existing_git_config() {
        let hooks = Path::new(r"C:\donnees\garde-fous\crochets");
        let env = git_guard_env(None, hooks);
        assert!(env.contains(&("GIT_CONFIG_COUNT".into(), "2".into())));
        assert!(env.contains(&("GIT_CONFIG_KEY_0".into(), "url.patronus-push-interdit:.pushInsteadOf".into())));
        assert!(env.contains(&("GIT_CONFIG_KEY_1".into(), "core.hooksPath".into())));
        assert!(env.contains(&("GIT_CONFIG_VALUE_1".into(), "C:/donnees/garde-fous/crochets".into())));
        assert!(env.contains(&("GIT_TERMINAL_PROMPT".into(), "0".into())));
        let env = git_guard_env(Some("2"), hooks);
        assert!(env.contains(&("GIT_CONFIG_COUNT".into(), "4".into())));
        assert!(env.iter().any(|(k, _)| k == "GIT_CONFIG_KEY_2"));
        assert!(env.iter().any(|(k, _)| k == "GIT_CONFIG_KEY_3"));
    }

    /// Un dépôt fictif `contoso-api` et git avec l'environnement de la session : rend la sortie et la
    /// réussite. Rien n'est jamais poussé : `git remote get-url --push` montre l'adresse que git
    /// utiliserait, `git hook run pre-push` lance le crochet que lancerait un push.
    fn guarded_git(name: &str) -> impl Fn(&[&str]) -> (String, bool) {
        let dir = crate::fsutil::test_dir(name);
        let hooks = guard_hooks(&dir.join("donnees")).unwrap();
        let repo = dir.join("contoso-api");
        std::fs::create_dir_all(&repo).unwrap();
        move |args: &[&str]| {
            let out =
                Command::new("git").arg("-C").arg(&repo).args(args).envs(git_guard_env(None, &hooks)).output().unwrap();
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            (text.trim().to_string(), out.status.success())
        }
    }

    /// Avec l'environnement de la session, git réécrit toute adresse de push et garde celle de lecture.
    #[test]
    fn push_url_is_rewritten_in_the_session_environment() {
        let git = guarded_git("push-guard");
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://contoso.com/api.git"]);
        assert_eq!(
            git(&["remote", "get-url", "--push", "origin"]).0,
            "patronus-push-interdit:https://contoso.com/api.git"
        );
        assert_eq!(git(&["remote", "get-url", "origin"]).0, "https://contoso.com/api.git");
    }

    /// Un `pushurl` explicite n'est pas réécrit par git : le crochet `pre-push` refuse quand même.
    #[test]
    fn explicit_pushurl_is_stopped_by_the_hook() {
        let git = guarded_git("push-guard-pushurl");
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://contoso.com/api.git"]);
        git(&["config", "remote.origin.pushurl", "https://contoso.com/api-push.git"]);
        assert_eq!(git(&["remote", "get-url", "--push", "origin"]).0, "https://contoso.com/api-push.git");
        let (text, ok) = git(&["hook", "run", "pre-push", "--", "origin", "https://contoso.com/api-push.git"]);
        assert!(!ok, "le crochet refuse : {text}");
        assert!(text.contains("Patronus : pas de push"), "{text}");
    }

    #[test]
    fn prompt_lists_the_changed_files() {
        let ctx = Context { trigger: "x".into(), files: vec![PathBuf::from("docs/guide.md")], ..Default::default() };
        let p = prompt(&claude_task(), &ctx);
        assert!(p.starts_with("Relis le code."));
        assert!(p.contains("- docs/guide.md"));
    }

    #[test]
    fn stream_is_summarised() {
        // Une réponse en deux blocs (même id, même usage), puis une seconde réponse.
        let text = r#"{"type":"system","subtype":"init","session_id":"abc"}
{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1000,"output_tokens":1},"content":[{"type":"text","text":"Je regarde."}]}}
{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1000,"output_tokens":1},"content":[{"type":"tool_use","name":"Read","input":{"file_path":"Cargo.toml"}}]}}
pas du json
{"type":"assistant","message":{"id":"m2","model":"claude-opus-5-5","usage":{"input_tokens":2000,"cache_read_input_tokens":10000,"output_tokens":1},"content":[{"type":"text","text":"Fini."}]}}"#;
        let mut s = parse_stream(text);
        assert_eq!(s.turns(), 2, "un tour par message, pas par bloc");
        assert_eq!(s.session_id.as_deref(), Some("abc"));
        assert!(!s.finished);
        assert_eq!(s.transcript[1], r#"→ Read {"file_path":"Cargo.toml"}"#);
        // 3 000 tokens d'entrée à 4 $, 10 000 lus en cache à 0,20 $, sortie minorée d'après le texte.
        let est = s.estimate().unwrap();
        assert!(est > 0.014 && est < 0.0145, "{est}");
        assert!(s.final_cost().unwrap().estime);
        s.feed(r#"{"type":"result","subtype":"success","is_error":false,"result":"Deux mises à jour mineures.","total_cost_usd":0.0421,"usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":25},"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"rm -rf x"}}]}"#);
        assert!(s.finished);
        assert_eq!(s.result.as_deref(), Some("Deux mises à jour mineures."));
        assert_eq!(s.final_cost(), Some(Cost { tokens_entree: 100, tokens_sortie: 25, usd: 0.0421, estime: false }));
        assert_eq!(s.denials, [Denial { tool: "Bash".into(), input: r#"{"command":"rm -rf x"}"#.into() }]);
    }

    #[test]
    fn unknown_model_is_not_estimated() {
        let s = parse_stream(
            r#"{"type":"assistant","message":{"id":"m1","model":"modele-inconnu","usage":{"input_tokens":5},"content":[]}}"#,
        );
        assert_eq!(s.estimate(), None);
    }

    #[test]
    fn reader_reads_only_what_was_added() {
        let dir = crate::fsutil::test_dir("stream-reader");
        let path = dir.join("sortie.jsonl");
        let line = |id: &str| {
            format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-5-5","content":[]}}}}"#)
        };
        std::fs::write(&path, format!("{}\n{}", line("a"), &line("b")[..20])).unwrap();
        let mut r = StreamReader::new(&path);
        assert_eq!(r.poll().turns(), 1, "la ligne incomplète attend");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        write!(f, "{}\n{}", &line("b")[20..], line("c")).unwrap();
        assert_eq!(r.poll().turns(), 2);
        assert_eq!(r.finish().turns(), 3, "la dernière ligne sans fin de ligne est lue à la fin");
    }

    #[test]
    fn claude_is_found_like_windows_does() {
        let base = crate::fsutil::test_dir("claude-path");
        let (npm, native, both) = (base.join("npm"), base.join("local"), base.join("both"));
        for d in [&npm, &native, &both] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(npm.join("claude.cmd"), "").unwrap();
        std::fs::write(native.join("claude.exe"), "").unwrap();
        std::fs::write(both.join("claude.cmd"), "").unwrap();
        std::fs::write(both.join("claude.exe"), "").unwrap();
        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        assert_eq!(find_in_path(Some(&path(&[&npm, &native]))), Some(npm.join("claude.cmd")), "ordre du PATH");
        assert_eq!(find_in_path(Some(&path(&[&both]))), Some(both.join("claude.exe")), ".exe avant .cmd");
        assert_eq!(find_in_path(Some(&path(&[&base]))), None);
        assert_eq!(find_in_path(None), None);
    }

    #[test]
    fn worktree_needs_a_repo() {
        let mut t = claude_task();
        t.action.worktree = Some(Worktree::on("main"));
        assert!(t.validate().join(" ").contains("worktree demande un dépôt"));
    }
}
