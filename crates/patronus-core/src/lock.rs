//! Verrou d'une tâche entre processus : une seule exécution à la fois, que la tâche parte de la
//! fenêtre, de la ligne de commande ou du Planificateur. Fichier `<données>\verrous\<tâche>.lock`,
//! créé de façon exclusive, qui nomme son processus ; un verrou laissé par un processus disparu est
//! repris.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::run::Owner;

pub struct TaskLock {
    path: PathBuf,
}

fn lock_path(dir: &Path, task_id: &str) -> PathBuf {
    dir.join(format!("{task_id}.lock"))
}

/// Le processus qui tient le verrou de la tâche, s'il vit encore.
pub fn holder(dir: &Path, task_id: &str) -> Option<Owner> {
    let text = fs::read_to_string(lock_path(dir, task_id)).ok()?;
    serde_json::from_str::<Owner>(&text).ok().filter(Owner::is_alive)
}

impl TaskLock {
    /// Prend le verrou, ou rend le processus qui le tient.
    pub fn acquire(dir: &Path, task_id: &str) -> Result<TaskLock, Owner> {
        let path = lock_path(dir, task_id);
        let _ = fs::create_dir_all(dir);
        let me = Owner::current();
        for _ in 0..3 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = f.write_all(serde_json::to_string(&me).unwrap_or_default().as_bytes());
                    return Ok(TaskLock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match holder(dir, task_id) {
                    Some(owner) => return Err(owner),
                    // Processus disparu, ou fichier en cours d'écriture : on réessaie.
                    None => {
                        let _ = fs::remove_file(&path);
                    }
                },
                // Dossier inaccessible : pas de verrou plutôt que pas d'exécution.
                Err(_) => return Ok(TaskLock { path: PathBuf::new() }),
            }
        }
        Err(holder(dir, task_id).unwrap_or(me))
    }
}

impl Drop for TaskLock {
    fn drop(&mut self) {
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
}
