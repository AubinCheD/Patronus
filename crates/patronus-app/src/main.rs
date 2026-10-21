//! Patronus — agents autonomes et tâches récurrentes pour le développeur.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Nom du verrou d'instance unique, propre au dossier de données : une instance d'essai
/// (`PATRONUS_DATA_DIR`) tourne à côté de celle de tous les jours.
fn instance_name(data_dir: &std::path::Path) -> String {
    format!("Local\\Patronus-single-instance:{}", data_dir.to_string_lossy().replace('\\', "/").to_lowercase())
}

/// Deux instances sur le même dossier lanceraient chaque tâche deux fois.
fn single_instance(data_dir: &std::path::Path) -> bool {
    let name = wide(&instance_name(data_dir));
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // Le handle reste ouvert jusqu'à la fin du processus.
        handle.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// `patronus lancer <tâche>` : exécute la tâche sans fenêtre, attend sa fin et affiche le résultat.
/// Le résultat arrive dans la boîte comme les autres.
fn run_cli(data_dir: &std::path::Path, task_id: &str) -> std::process::ExitCode {
    use patronus_core::{engine::Engine, exec::Context, run::now};
    // Exécutable sans console en version finale : on reprend celle du terminal appelant.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS)
    };
    let mut engine = Engine::open(data_dir, now());
    if engine.task(task_id).is_none() {
        let known: Vec<_> = engine.loaded.iter().map(|l| l.id.as_str()).collect();
        eprintln!("Tâche inconnue : {task_id}. Tâches : {}", known.join(", "));
        return std::process::ExitCode::from(2);
    }
    engine.launch(task_id, Context { trigger: "ligne de commande".into(), files: vec![] }, now());
    while engine.is_running(task_id) {
        std::thread::sleep(std::time::Duration::from_millis(100));
        engine.tick(now());
    }
    let Some(run) = engine.runs.iter().find(|r| r.tache == task_id) else { return std::process::ExitCode::FAILURE };
    println!("{} : {}\n{}", run.tache_nom, run.statut.label(), run.resume);
    if run.statut.is_ok() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

fn main() -> std::process::ExitCode {
    let data_dir = patronus_core::fsutil::data_dir();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, id] if cmd == "lancer" => return run_cli(&data_dir, id),
        [] => {}
        _ => {
            eprintln!("Usage : patronus [lancer <tâche>]");
            return std::process::ExitCode::from(2);
        }
    }
    match gui(data_dir) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn gui(data_dir: std::path::PathBuf) -> eframe::Result {
    if !single_instance(&data_dir) {
        eprintln!("Patronus tourne déjà sur {}.", data_dir.display());
        return Ok(());
    }
    let result = patronus_app::run(data_dir);
    patronus_app::notify::cleanup();
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn instance_name_has_no_backslash_after_namespace() {
        let n = super::instance_name(std::path::Path::new(r"C:\Temp\Patronus-Essai"));
        assert!(!n["Local\\".len()..].contains('\\'), "{n}");
        assert_eq!(n, super::instance_name(std::path::Path::new(r"c:\temp\patronus-essai")));
    }
}
