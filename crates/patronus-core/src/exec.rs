//! Lancement d'un processus surveillé : sortie dans le journal, durée maximale, arrêt à la demande
//! (arbre de processus compris).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::run::{Cost, Status, read_lossy, tail_summary};
use crate::task::{Script, Shell, Task};
use crate::winproc::{Job, resume_threads};

/// Pourquoi une exécution part : affiché dans la boîte et passé au script.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub trigger: String,
    /// Fichiers modifiés, pour le déclencheur « fichiers ».
    pub files: Vec<PathBuf>,
    /// Variables d'environnement données au script et à la session (`PATRONUS_BRANCHE`…).
    pub vars: Vec<(String, String)>,
    /// Ce que le déclencheur a à dire (commits arrivés…) : dans le journal, et à la fin de la
    /// consigne d'une session Claude.
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: Status,
    pub code: Option<i32>,
    pub summary: String,
    pub cost: Option<Cost>,
    pub worktree: Option<PathBuf>,
    /// Identifiant de la session Claude, pour la reprendre.
    pub session: Option<String>,
}

impl Outcome {
    pub fn impossible(why: impl Into<String>) -> Outcome {
        Outcome {
            status: Status::Impossible,
            code: None,
            summary: why.into(),
            cost: None,
            worktree: None,
            session: None,
        }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// Lance `cmd`, sortie standard et d'erreur ajoutées au journal, et attend sa fin, l'arrêt demandé
/// (`stop`) ou la durée maximale. Rend le statut et le code de retour.
pub fn run_process(cmd: Command, journal: &Path, stop: &AtomicBool, max: Duration) -> (Status, Option<i32>) {
    run_process_with(cmd, journal, None, None, stop, max, &mut || None)
}

/// Comme `run_process`, la sortie standard pouvant aller dans un fichier à part (`stdout`), l'entrée
/// standard venir d'un fichier (`stdin`), et `check` appelé régulièrement : s'il rend un statut et une
/// raison, le processus est arrêté.
pub fn run_process_with(
    mut cmd: Command,
    journal: &Path,
    stdout: Option<&Path>,
    stdin: Option<&Path>,
    stop: &AtomicBool,
    max: Duration,
    check: &mut dyn FnMut() -> Option<(Status, String)>,
) -> (Status, Option<i32>) {
    let journal_file = match OpenOptions::new().create(true).append(true).open(journal) {
        Ok(f) => Arc::new(Mutex::new(f)),
        Err(e) => {
            log(journal, &format!("[patronus] journal impossible : {e}"));
            return (Status::Impossible, None);
        }
    };
    // Sortie standard dans un fichier à part (flux de Claude, écrit tel quel), ou dans le journal ;
    // ce qui va au journal passe par un tube, décodé ligne à ligne (`decode_line`).
    let out = match stdout {
        Some(p) => match OpenOptions::new().create(true).append(true).open(p) {
            Ok(f) => Stdio::from(f),
            Err(e) => {
                log(journal, &format!("[patronus] sortie impossible : {e}"));
                return (Status::Impossible, None);
            }
        },
        None => Stdio::piped(),
    };
    let input = match stdin.map(std::fs::File::open) {
        None => Stdio::null(),
        Some(Ok(f)) => Stdio::from(f),
        Some(Err(e)) => {
            log(journal, &format!("[patronus] entrée standard illisible : {e}"));
            return (Status::Impossible, None);
        }
    };
    cmd.stdin(input).stdout(out).stderr(Stdio::piped());
    // Créé suspendu, rangé dans un objet de tâche, puis relancé : aucun petit-enfant n'échappe à
    // l'objet, et tout s'arrête avec lui (arrêt demandé, ou Patronus qui s'arrête brutalement).
    let job = Job::new();
    let suspended = job.is_some();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW | if suspended { CREATE_SUSPENDED } else { 0 });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(journal, &format!("[patronus] lancement impossible : {e}"));
            return (Status::Impossible, None);
        }
    };
    let job = job.filter(|j| j.assign(&child));
    let mut pumps = Vec::new();
    if let Some(o) = child.stdout.take() {
        pumps.push(pump(o, journal_file.clone()));
    }
    if let Some(e) = child.stderr.take() {
        pumps.push(pump(e, journal_file.clone()));
    }
    if suspended && resume_threads(child.id()) == 0 {
        log(journal, "[patronus] le processus n'a pas pu être relancé après sa création");
    }
    let result = supervise(&mut child, job.as_ref(), journal, stop, max, check);
    // La sortie est entièrement recopiée avant de rendre la main. Les processus laissés en arrière-plan
    // ont été arrêtés avec l'objet de tâche : les tubes se ferment aussitôt. L'attente n'est bornée que
    // pour un processus resté hors de l'objet (objet de tâche impossible), qui garderait le tube ouvert ;
    // elle est large, pour une machine chargée.
    for done in pumps {
        if done.recv_timeout(Duration::from_secs(10)).is_err() {
            log(journal, "[patronus] sortie peut-être incomplète : un processus garde le tube ouvert");
        }
    }
    result
}

/// Recopie une sortie dans le journal, ligne à ligne. Prévient sur le canal rendu quand la sortie est
/// fermée.
fn pump(source: impl std::io::Read + Send + 'static, journal: Arc<Mutex<fs::File>>) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(source).split(b'\n').map_while(Result::ok) {
            let text = decode_line(&line);
            let mut f = journal.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(f, "{}", text.trim_end_matches('\r'));
        }
        let _ = tx.send(());
    });
    rx
}

/// Une ligne de sortie : UTF-8 si elle l'est, sinon la page de code OEM (celle de cmd et de
/// nombreux outils Windows quand leur sortie ne va pas à une console).
pub fn decode_line(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => oem_to_string(bytes),
    }
}

#[cfg(windows)]
fn oem_to_string(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{CP_OEMCP, MultiByteToWideChar};
    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; n as usize];
        MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), n);
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(not(windows))]
fn oem_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Attend la fin du processus, l'arrêt demandé, la durée maximale ou ce que `check` signale.
fn supervise(
    child: &mut std::process::Child,
    job: Option<&Job>,
    journal: &Path,
    stop: &AtomicBool,
    max: Duration,
    check: &mut dyn FnMut() -> Option<(Status, String)>,
) -> (Status, Option<i32>) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let code = st.code();
                // Ce que le processus a laissé tourner derrière lui s'arrête avec l'objet de tâche.
                if let Some(j) = job {
                    let left = j.active_processes();
                    if left > 0 {
                        log(journal, &format!("[patronus] {left} processus laissé(s) en arrière-plan, arrêté(s)"));
                        j.terminate();
                    }
                }
                return (if st.success() { Status::Reussie } else { Status::Echouee }, code);
            }
            Ok(None) => {}
            Err(e) => {
                log(journal, &format!("[patronus] attente impossible : {e}"));
                kill_tree(child, job);
                return (Status::Echouee, None);
            }
        }
        let why = if stop.load(Ordering::Relaxed) {
            Some((Status::Arretee, "arrêtée à la main".to_string()))
        } else if start.elapsed() >= max {
            Some((Status::DureeDepassee, "durée maximale dépassée".to_string()))
        } else {
            check()
        };
        if let Some((status, msg)) = why {
            kill_tree(child, job);
            log(journal, &format!("\n[patronus] {msg}"));
            return (status, None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Arrête le processus et tout ce qu'il a lancé : par l'objet de tâche, sinon par `taskkill /T`
/// (qui ne suit que les liens de parenté encore vivants).
fn kill_tree(child: &mut std::process::Child, job: Option<&Job>) {
    match job {
        Some(j) => j.terminate(),
        None => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &child.id().to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .status();
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Ajoute une ligne au journal.
pub fn log(journal: &Path, line: &str) {
    if let Some(dir) = journal.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(journal) {
        let _ = writeln!(f, "{line}");
    }
}

/// Fichiers modifiés cités dans une variable d'environnement, et dans la consigne d'une session.
pub const MAX_FILES_IN_ENV: usize = 100;
pub const MAX_FILES_IN_PROMPT: usize = 50;

/// Les `max` premiers fichiers, un par ligne, puis « … et N autres » s'il y en a plus.
pub fn capped_list(files: &[PathBuf], max: usize) -> String {
    let mut lines: Vec<String> = files.iter().take(max).map(|f| f.display().to_string()).collect();
    if files.len() > max {
        lines.push(format!(
            "… et {} autres ({} au total, liste complète dans fichiers.txt)",
            files.len() - max,
            files.len()
        ));
    }
    lines.join(
        "
",
    )
}

/// Écrit la liste complète des fichiers modifiés dans `fichiers.txt` du dossier de l'exécution.
pub fn write_file_list(run_dir: &Path, files: &[PathBuf]) -> Option<PathBuf> {
    let path = run_dir.join("fichiers.txt");
    let text: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    crate::fsutil::write_atomic(
        &path,
        text.join(
            "
",
        )
        .as_bytes(),
    )
    .ok()
    .map(|_| path)
}

/// Dossier de travail : celui du script, à défaut le dépôt de la tâche, à défaut le dossier personnel.
pub fn work_dir(task: &Task, script: Option<&Script>) -> PathBuf {
    script
        .and_then(|s| s.dossier.clone())
        .or_else(|| task.depot.clone())
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Git Bash s'il est installé à sa place habituelle, sinon le `bash` du PATH.
fn bash_exe() -> PathBuf {
    for base in [std::env::var_os("ProgramFiles"), std::env::var_os("ProgramW6432")].into_iter().flatten() {
        let p = PathBuf::from(base).join("Git").join("bin").join("bash.exe");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("bash")
}

/// La commande du shell pour un script.
pub fn shell_command(script: &Script) -> Command {
    match script.shell {
        Shell::Powershell => {
            let mut c = Command::new("powershell.exe");
            c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
            // Sortie en UTF-8, pour un journal lisible.
            c.arg(format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {}", script.commande));
            c
        }
        Shell::Cmd => {
            let mut c = Command::new("cmd.exe");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // cmd a ses propres règles de guillemets : la commande passe telle quelle.
                c.raw_arg(format!("/D /S /C \"{}\"", script.commande));
            }
            #[cfg(not(windows))]
            c.args(["/C", &script.commande]);
            c
        }
        Shell::Bash => {
            let mut c = Command::new(bash_exe());
            c.args(["-c", &script.commande]);
            c
        }
    }
}

/// Exécute l'action script d'une tâche ; tout ce qui se passe va dans le journal.
pub fn run_script(task_id: &str, task: &Task, ctx: &Context, journal: &Path, stop: &AtomicBool) -> Outcome {
    let Some(script) = &task.action.script else { return Outcome::impossible("La tâche n'a pas de script.") };
    let dir = work_dir(task, Some(script));
    log(journal, &format!("[patronus] {} — {} ({})", task.nom, ctx.trigger, dir.display()));
    log(journal, &format!("[patronus] {} > {}", script.shell.label(), script.commande));
    if !dir.is_dir() {
        let msg = format!("Dossier de travail introuvable : {}", dir.display());
        log(journal, &format!("[patronus] {msg}"));
        return Outcome::impossible(msg);
    }
    let mut cmd = shell_command(script);
    cmd.current_dir(&dir).envs(&script.env);
    cmd.env("PATRONUS_TACHE", task_id).env("PATRONUS_DECLENCHEUR", &ctx.trigger);
    cmd.envs(ctx.vars.iter().map(|(k, v)| (k, v)));
    for line in ctx.notes.lines() {
        log(journal, &format!("[patronus] {line}"));
    }
    if !ctx.files.is_empty() {
        // Une variable d'environnement est limitée en taille : la liste complète est dans un fichier.
        cmd.env("PATRONUS_FICHIERS", capped_list(&ctx.files, MAX_FILES_IN_ENV));
        if let Some(list) = journal.parent().and_then(|d| write_file_list(d, &ctx.files)) {
            cmd.env("PATRONUS_FICHIERS_LISTE", list);
        }
    }
    let max = Duration::from_secs(task.garde_fous.duree_max_min as u64 * 60);
    let (status, code) = run_process(cmd, journal, stop, max);
    let output = read_lossy(journal);
    let body: String = output.lines().filter(|l| !l.starts_with("[patronus]")).collect::<Vec<_>>().join("\n");
    let mut summary = tail_summary(&body, 5, 400);
    if summary.is_empty() {
        summary = match status {
            Status::Reussie => "Terminé, sans sortie.".into(),
            _ => String::new(),
        };
    }
    match status {
        Status::Echouee => {
            summary = format!("Code de retour {}.\n{summary}", code.map_or("inconnu".into(), |c| c.to_string()))
        }
        Status::DureeDepassee => {
            summary = format!("Arrêtée : durée maximale ({} min) dépassée.\n{summary}", task.garde_fous.duree_max_min)
        }
        Status::Arretee => summary = format!("Arrêtée à la main.\n{summary}"),
        Status::Impossible if summary.is_empty() => {
            summary = "Le shell n'a pas pu être lancé (voir le journal).".into()
        }
        _ => {}
    }
    Outcome { status, code, summary: summary.trim().to_string(), cost: None, worktree: None, session: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;

    fn script_task(shell: Shell, commande: &str, dir: &Path) -> Task {
        let mut t = Task::new_script("Essai");
        t.action.script =
            Some(Script { commande: commande.into(), shell, dossier: Some(dir.to_path_buf()), ..Default::default() });
        t
    }

    fn ctx() -> Context {
        Context { trigger: "à la main".into(), files: vec![], ..Default::default() }
    }

    #[test]
    fn cmd_script_output_goes_to_journal_and_summary() {
        let dir = test_dir("exec-cmd");
        let journal = dir.join("journal.txt");
        let mut t = script_task(Shell::Cmd, "echo bonjour contoso && echo %PATRONUS_TACHE%", &dir);
        t.action.script.as_mut().unwrap().env.insert("X".into(), "1".into());
        let out = run_script("essai", &t, &ctx(), &journal, &AtomicBool::new(false));
        assert_eq!(out.status, Status::Reussie, "{}", read_lossy(&journal));
        assert_eq!(out.code, Some(0));
        assert!(out.summary.contains("bonjour contoso"), "{}", out.summary);
        assert!(out.summary.contains("essai"));
        assert!(read_lossy(&journal).contains("[patronus] cmd > echo"));
    }

    #[test]
    fn failure_keeps_the_exit_code() {
        let dir = test_dir("exec-fail");
        let t = script_task(Shell::Cmd, "echo perdu 1>&2 && exit /b 3", &dir);
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Echouee);
        assert_eq!(out.code, Some(3));
        assert!(out.summary.contains("Code de retour 3") && out.summary.contains("perdu"), "{}", out.summary);
    }

    #[test]
    fn powershell_script_runs() {
        let dir = test_dir("exec-ps");
        let t = script_task(Shell::Powershell, "Write-Output \"fabrikam é\"", &dir);
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Reussie, "{}", out.summary);
        assert!(out.summary.contains("fabrikam é"), "{}", out.summary);
    }

    #[test]
    fn stop_kills_the_whole_tree() {
        let dir = test_dir("exec-stop");
        let t = script_task(Shell::Cmd, "ping -n 30 127.0.0.1 >nul", &dir);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            s2.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &stop);
        assert_eq!(out.status, Status::Arretee);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn duration_limit_stops_the_run() {
        let dir = test_dir("exec-timeout");
        let t = script_task(Shell::Cmd, "ping -n 30 127.0.0.1 >nul", &dir);
        let mut cmd = shell_command(t.action.script.as_ref().unwrap());
        cmd.current_dir(&dir);
        let (status, _) = run_process(cmd, &dir.join("j.txt"), &AtomicBool::new(false), Duration::from_millis(500));
        assert_eq!(status, Status::DureeDepassee);
        assert!(read_lossy(&dir.join("j.txt")).contains("durée maximale"));
    }

    #[test]
    fn missing_work_dir_is_impossible_and_says_why() {
        let dir = test_dir("exec-missing");
        let t = script_task(Shell::Cmd, "echo x", &dir.join("absent"));
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Impossible);
        assert!(out.summary.contains("introuvable"));
    }

    #[test]
    fn changed_files_are_passed_to_the_script() {
        let dir = test_dir("exec-files");
        let t = script_task(Shell::Cmd, "echo %PATRONUS_FICHIERS%", &dir);
        let c = Context {
            trigger: "fichiers".into(),
            files: vec![PathBuf::from(r"C:\dev\northwind\a.md")],
            ..Default::default()
        };
        let out = run_script("e", &t, &c, &dir.join("j.txt"), &AtomicBool::new(false));
        assert!(out.summary.contains("northwind"), "{}", out.summary);
    }

    /// Beaucoup de fichiers modifiés : la variable en cite 100, la liste complète est à côté.
    #[test]
    fn changed_files_are_capped_in_the_environment() {
        let dir = test_dir("exec-many-files");
        let t = script_task(
            Shell::Powershell,
            "Write-Output ($env:PATRONUS_FICHIERS -split \"`n\").Count; Get-Content $env:PATRONUS_FICHIERS_LISTE | Measure-Object -Line | ForEach-Object Lines",
            &dir,
        );
        let files: Vec<PathBuf> = (0..500).map(|i| PathBuf::from(format!(r"C:\dev\northwind\f{i}.txt"))).collect();
        let c = Context { trigger: "fichiers".into(), files, ..Default::default() };
        let out = run_script("e", &t, &c, &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Reussie, "{}", out.summary);
        let lines: Vec<&str> = out.summary.lines().collect();
        assert_eq!(lines, ["101", "500"], "100 fichiers + la ligne « … et 400 autres », puis la liste complète");
    }

    /// Attend que le script ait écrit le numéro du processus qu'il a lancé.
    fn wait_pid(file: &Path) -> u32 {
        let start = Instant::now();
        loop {
            if let Some(pid) = fs::read_to_string(file).ok().and_then(|s| s.trim().parse().ok()) {
                return pid;
            }
            assert!(start.elapsed() < Duration::from_secs(15), "le script n'a pas écrit {}", file.display());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn dies_soon(pid: u32) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if crate::winproc::created(pid).is_none() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Un petit-enfant (lancé par le script, hors de toute parenté que `taskkill /T` suivrait) meurt
    /// avec l'arrêt.
    #[test]
    fn grandchild_dies_with_the_stop() {
        let dir = test_dir("exec-grandchild");
        let pid_file = dir.join("pid.txt");
        let t = script_task(
            Shell::Powershell,
            &format!(
                "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru; \
                 Set-Content -Path '{}' -Value $p.Id; Start-Sleep -Seconds 60",
                pid_file.display()
            ),
            &dir,
        );
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let pf = pid_file.clone();
        let waiter = std::thread::spawn(move || {
            let pid = wait_pid(&pf);
            s2.store(true, Ordering::Relaxed);
            pid
        });
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &stop);
        let pid = waiter.join().unwrap();
        assert_eq!(out.status, Status::Arretee);
        assert!(dies_soon(pid), "le ping {pid} lancé par le script tourne encore");
    }

    /// Ce qu'un script laisse tourner en arrière-plan s'arrête avec lui, et c'est noté.
    #[test]
    fn leftover_background_process_is_stopped() {
        let dir = test_dir("exec-leftover");
        let pid_file = dir.join("pid.txt");
        let t = script_task(
            Shell::Powershell,
            &format!(
                "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden -PassThru; \
                 Set-Content -Path '{}' -Value $p.Id",
                pid_file.display()
            ),
            &dir,
        );
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Reussie, "{}", out.summary);
        let pid = wait_pid(&pid_file);
        assert!(dies_soon(pid), "le ping {pid} laissé par le script tourne encore");
        assert!(read_lossy(&dir.join("j.txt")).contains("laissé(s) en arrière-plan, arrêté(s)"));
    }

    /// La sortie de cmd est en page de code OEM (« caf� ») : elle est forcée en UTF-8.
    #[test]
    fn cmd_output_is_utf8() {
        let dir = test_dir("exec-cmd-utf8");
        let t = script_task(Shell::Cmd, "echo café élève", &dir);
        let out = run_script("e", &t, &ctx(), &dir.join("j.txt"), &AtomicBool::new(false));
        assert_eq!(out.status, Status::Reussie, "{}", out.summary);
        assert!(out.summary.contains("café élève"), "{}", out.summary);
    }
}
