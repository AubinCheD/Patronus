//! Patronus — agents autonomes et tâches récurrentes pour le développeur.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Nom du verrou d'instance unique, propre au dossier de données : une instance d'essai
/// (`PATRONUS_DATA_DIR`, `--donnees`) tourne à côté de celle de tous les jours.
fn instance_name(data_dir: &Path) -> String {
    let dir = data_dir.to_string_lossy().replace('\\', "/");
    format!("Local\\Patronus-single-instance:{}", dir.trim_end_matches('/').to_lowercase())
}

/// Deux fenêtres sur le même dossier lanceraient chaque tâche deux fois.
fn single_instance(data_dir: &Path) -> bool {
    let name = wide(&instance_name(data_dir));
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // Le handle reste ouvert jusqu'à la fin du processus.
        handle.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Ce que demande la ligne de commande.
#[derive(Debug, PartialEq)]
enum Cli {
    /// La fenêtre ; réduite (`--reduit`) au lancement avec Windows.
    Window { minimized: bool },
    /// `lancer <tâche>` : exécute la tâche sans fenêtre et attend sa fin.
    Run(String),
    /// `planifie <tâche>` : lancé par le Planificateur de Windows à une échéance.
    Planned(String),
}

/// `patronus [--donnees <dossier>] [--reduit | lancer <tâche> | planifie <tâche>]`.
fn parse(args: &[String]) -> Result<(Option<PathBuf>, Cli), String> {
    let mut data_dir = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--donnees" => data_dir = Some(PathBuf::from(it.next().ok_or("--donnees attend un dossier")?)),
            _ => rest.push(a.as_str()),
        }
    }
    let cli = match rest.as_slice() {
        [] => Cli::Window { minimized: false },
        ["--reduit"] => Cli::Window { minimized: true },
        ["lancer", id] => Cli::Run(id.to_string()),
        ["planifie", id] => Cli::Planned(id.to_string()),
        _ => return Err(format!("arguments inconnus : {}", rest.join(" "))),
    };
    Ok((data_dir, cli))
}

/// `patronus lancer <tâche>` : exécute la tâche sans fenêtre (rien d'autre : ni échéances, ni
/// surveillance), attend sa fin et affiche le résultat, qui arrive aussi dans la boîte. Refuse si la
/// tâche tourne déjà, ici ou dans la fenêtre (code 3).
fn run_cli(data_dir: &Path, task_id: &str) -> ExitCode {
    use patronus_core::{engine::Engine, exec::Context, run::now};
    let mut engine = Engine::open_headless(data_dir, now());
    if engine.task(task_id).is_none() {
        let known: Vec<_> = engine.loaded.iter().map(|l| l.id.as_str()).collect();
        eprintln!("Tâche inconnue : {task_id}. Tâches : {}", known.join(", "));
        return ExitCode::from(2);
    }
    if let Err(why) = engine.try_launch(
        task_id,
        Context { trigger: "ligne de commande".into(), files: vec![], ..Default::default() },
        now(),
    ) && engine.running_elsewhere(task_id).is_some()
    {
        eprintln!("{why}");
        return ExitCode::from(3);
    }
    while engine.is_running(task_id) {
        std::thread::sleep(std::time::Duration::from_millis(100));
        engine.tick(now());
    }
    let Some(run) = engine.runs.iter().find(|r| r.tache == task_id) else { return ExitCode::FAILURE };
    println!("{} : {}\n{}", run.tache_nom, run.statut.label(), run.resume);
    if run.statut.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// La fenêtre de Patronus tourne-t-elle sur ce dossier de données ? (son verrou d'instance existe)
fn window_running(data_dir: &Path) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name = wide(&instance_name(data_dir));
    unsafe {
        let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// Une ligne dans `planificateur.log` : ce que chaque passage du Planificateur a décidé, même quand
/// il n'y avait rien à faire. Le fichier est ramené à sa seconde moitié au-delà de 256 Ko.
fn planner_log(data_dir: &Path, task_id: &str, text: &str) {
    use std::io::Write;
    let path = data_dir.join("planificateur.log");
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > 256 * 1024
        && let Ok(old) = std::fs::read_to_string(&path)
    {
        let half = old.len() / 2;
        let cut = old[half..].find('\n').map_or(half, |i| half + i + 1);
        let _ = std::fs::write(&path, &old[cut..]);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {task_id} : {text}", patronus_core::run::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// `patronus planifie <tâche>`, lancé par le Planificateur de Windows à une échéance : rien à faire si
/// la fenêtre est ouverte (elle tient ses échéances) ou si l'échéance a déjà été tenue ; sinon la
/// tâche part, sans fenêtre, et son résultat arrive dans la boîte.
fn run_planned(data_dir: &Path, task_id: &str) -> ExitCode {
    use patronus_core::{engine::Engine, exec::Context, run::now};
    if window_running(data_dir) {
        planner_log(data_dir, task_id, "fenêtre ouverte : elle tient elle-même ses échéances");
        return ExitCode::SUCCESS;
    }
    let mut engine = Engine::open_headless(data_dir, now());
    let at = match engine.planned_run(task_id, now()) {
        Ok(at) => at,
        Err(why) => {
            planner_log(data_dir, task_id, &format!("rien à faire ({why})"));
            return ExitCode::SUCCESS;
        }
    };
    let trigger = format!("Planificateur de Windows (échéance de {})", at.format("%H:%M"));
    if let Err(why) = engine.try_launch(task_id, Context { trigger, ..Default::default() }, now()) {
        planner_log(data_dir, task_id, &format!("non lancée : {why}"));
        return ExitCode::from(3);
    }
    while engine.is_running(task_id) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        engine.tick(now());
    }
    let statut = engine.runs.iter().find(|r| r.tache == task_id).map(|r| r.statut);
    planner_log(data_dir, task_id, &format!("exécutée : {}", statut.map_or("?", |s| s.label())));
    if statut.is_some_and(|s| s.is_ok()) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let console = !args.is_empty();
    if console {
        // Exécutable sans console en version finale : on reprend celle du terminal appelant.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            )
        };
    }
    let (dir, cli) = match parse(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}\nUsage : patronus [--donnees <dossier>] [--reduit | lancer <tâche>]");
            return ExitCode::from(2);
        }
    };
    let data_dir = dir.unwrap_or_else(patronus_core::fsutil::data_dir);
    match cli {
        Cli::Run(id) => run_cli(&data_dir, &id),
        Cli::Planned(id) => run_planned(&data_dir, &id),
        Cli::Window { minimized } => match gui(data_dir, minimized) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn gui(data_dir: PathBuf, minimized: bool) -> eframe::Result {
    if !single_instance(&data_dir) {
        eprintln!("Patronus tourne déjà sur {}.", data_dir.display());
        return Ok(());
    }
    let result = patronus_app::run(data_dir, minimized);
    patronus_app::notify::cleanup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn instance_name_has_no_backslash_after_namespace() {
        let n = instance_name(Path::new(r"C:\Temp\Patronus-Essai"));
        assert!(!n["Local\\".len()..].contains('\\'), "{n}");
        assert_eq!(n, instance_name(Path::new(r"c:\temp\patronus-essai\")));
    }

    #[test]
    fn command_line() {
        assert_eq!(parse(&[]), Ok((None, Cli::Window { minimized: false })));
        assert_eq!(
            parse(&args(&["--donnees", r"C:\essai", "--reduit"])),
            Ok((Some(PathBuf::from(r"C:\essai")), Cli::Window { minimized: true }))
        );
        assert_eq!(parse(&args(&["lancer", "revue"])), Ok((None, Cli::Run("revue".into()))));
        assert_eq!(
            parse(&args(&["--donnees", r"C:\essai", "lancer", "revue"])),
            Ok((Some(PathBuf::from(r"C:\essai")), Cli::Run("revue".into())))
        );
        assert!(parse(&args(&["--donnees"])).is_err());
        assert!(parse(&args(&["lancer"])).is_err());
        assert!(parse(&args(&["inconnu"])).is_err());
        assert_eq!(parse(&args(&["planifie", "matin"])), Ok((None, Cli::Planned("matin".into()))));
    }
}
