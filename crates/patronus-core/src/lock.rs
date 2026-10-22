//! Verrou d'une tâche entre processus : une seule exécution à la fois, que la tâche parte de la
//! fenêtre, de la ligne de commande ou du Planificateur. Fichier `<données>\verrous\<tâche>.lock`,
//! créé de façon exclusive, qui nomme son processus ; un verrou laissé par un processus disparu est
//! repris.
//!
//! Le fichier reste ouvert, sans partage en suppression, tant que le verrou est tenu : personne ne
//! peut l'effacer pendant que son processus vit (même avant qu'il y ait écrit son nom), et Windows le
//! libère à la mort du processus. Reprendre un verrou abandonné ne peut donc pas effacer celui qu'un
//! autre processus vient de prendre.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::run::Owner;

pub struct TaskLock {
    path: PathBuf,
    file: Option<File>,
}

fn lock_path(dir: &Path, task_id: &str) -> PathBuf {
    dir.join(format!("{task_id}.lock"))
}

/// Le processus qui tient le verrou de la tâche, s'il vit encore.
pub fn holder(dir: &Path, task_id: &str) -> Option<Owner> {
    let text = fs::read_to_string(lock_path(dir, task_id)).ok()?;
    serde_json::from_str::<Owner>(&text).ok().filter(Owner::is_alive)
}

/// Crée le fichier du verrou, exclusif ; les autres peuvent le lire, pas l'effacer.
fn create(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    o.open(path)
}

impl TaskLock {
    /// Prend le verrou, ou rend le processus qui le tient.
    pub fn acquire(dir: &Path, task_id: &str) -> Result<TaskLock, Owner> {
        let path = lock_path(dir, task_id);
        let _ = fs::create_dir_all(dir);
        let me = Owner::current();
        for _ in 0..20 {
            match create(&path) {
                Ok(mut f) => {
                    let _ = f.write_all(serde_json::to_string(&me).unwrap_or_default().as_bytes());
                    let _ = f.flush();
                    return Ok(TaskLock { path, file: Some(f) });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(owner) = holder(dir, task_id) {
                        return Err(owner);
                    }
                    // Processus disparu : le fichier s'efface. Encore ouvert (verrou tenu, en cours
                    // d'écriture) : il ne s'efface pas, et on relit un peu plus tard.
                    if fs::remove_file(&path).is_err() {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                // Dossier inaccessible : pas de verrou plutôt que pas d'exécution.
                Err(_) => return Ok(TaskLock { path: PathBuf::new(), file: None }),
            }
        }
        Err(holder(dir, task_id).unwrap_or(me))
    }
}

impl Drop for TaskLock {
    fn drop(&mut self) {
        // Fermer d'abord : le fichier n'est effaçable qu'une fois fermé.
        self.file = None;
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;

    #[test]
    fn one_holder_at_a_time_and_released_on_drop() {
        let dir = test_dir("lock");
        let a = TaskLock::acquire(&dir, "revue").unwrap();
        assert_eq!(TaskLock::acquire(&dir, "revue").err(), Some(Owner::current()));
        assert!(TaskLock::acquire(&dir, "autre").is_ok());
        drop(a);
        assert!(holder(&dir, "revue").is_none());
        assert!(TaskLock::acquire(&dir, "revue").is_ok());
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let dir = test_dir("lock-stale");
        let dead = Owner { pid: std::process::id(), cree: 1 };
        fs::write(dir.join("revue.lock"), serde_json::to_string(&dead).unwrap()).unwrap();
        assert!(holder(&dir, "revue").is_none());
        assert!(TaskLock::acquire(&dir, "revue").is_ok());
    }

    /// Un verrou tout juste créé, pas encore écrit (illisible), n'est pas pris pour un abandon : son
    /// fichier, encore ouvert par son processus, ne s'efface pas.
    #[test]
    fn a_lock_being_written_is_not_taken_over() {
        let dir = test_dir("lock-ecriture");
        let path = dir.join("revue.lock");
        let held = create(&path).unwrap();
        assert!(TaskLock::acquire(&dir, "revue").is_err());
        assert!(path.exists(), "pas effacé");
        drop(held);
        assert!(TaskLock::acquire(&dir, "revue").is_ok(), "fermé sans nom : abandonné, repris");
    }
}
