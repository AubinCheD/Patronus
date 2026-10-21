//! Déclencheur « branche distante » : `git ls-remote` lit le commit d'une branche distante sans rien
//! écrire ; quand il change, `git fetch` récupère les nouveaux commits (objets et branche de suivi
//! `origin/main`, jamais la copie de travail ni les branches locales) et `git log` les liste.
//!
//! Git est lancé sans fenêtre, sans jamais demander d'identifiants (une demande bloquerait), et au
//! plus `GIT_TIMEOUT` : un réseau absent ne fige rien.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Lance git dans `repo` ; rend sa sortie, ou son message d'erreur.
fn git(repo: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("git introuvable : {e}"))?;
    let read = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = r.read_to_end(&mut s);
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = read(Box::new(child.stdout.take().expect("sortie")));
    let err = read(Box::new(child.stderr.take().expect("erreurs")));
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("git {} : pas de réponse en {} s", args.first().unwrap_or(&""), timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() { Ok(out) } else { Err(err.trim().to_string()) }
}

/// Commit actuel de la branche distante, lu sans rien écrire dans le dépôt.
pub fn head(repo: &Path, remote: &str, branch: &str) -> Result<String, String> {
    let refname = format!("refs/heads/{branch}");
    let out = git(repo, &["ls-remote", "--end-of-options", remote, &refname], GIT_TIMEOUT)?;
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(_, r)| *r == refname)
        .map(|(sha, _)| sha.trim().to_string())
        .ok_or_else(|| format!("branche {branch} absente de {remote}"))
}

/// Récupère les commits de la branche distante (objets et branche de suivi seulement).
pub fn fetch(repo: &Path, remote: &str, branch: &str) -> Result<(), String> {
    git(repo, &["fetch", "--quiet", "--no-tags", "--end-of-options", remote, branch], GIT_TIMEOUT).map(|_| ())
}

/// Les commits de `from` (exclu) à `to`, du plus récent au plus ancien, `max` au plus : « abc1234
/// Auteur : sujet ». Si `from` est inconnu (historique réécrit), les derniers commits de `to`.
pub fn log(repo: &Path, from: &str, to: &str, max: usize) -> Vec<String> {
    let n = format!("-{max}");
    let fmt = "--format=%h %an : %s";
    git(repo, &["log", &n, fmt, &format!("{from}..{to}"), "--"], GIT_TIMEOUT)
        .or_else(|_| git(repo, &["log", &n, fmt, to, "--"], GIT_TIMEOUT))
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Ce qu'un relevé de branche a donné.
#[derive(Clone, Debug, PartialEq)]
pub enum Poll {
    /// Premier relevé : commit retenu, sans départ.
    First(String),
    Same,
    /// La branche a avancé (ou changé) : commits récupérés.
    Moved {
        from: String,
        to: String,
        log: Vec<String>,
    },
    Failed(String),
}

/// Relève la branche distante ; `known` est le dernier commit vu. Lent (réseau) : à lancer hors du
/// fil de l'interface.
pub fn poll(repo: &Path, remote: &str, branch: &str, known: Option<&str>) -> Poll {
    let to = match head(repo, remote, branch) {
        Ok(sha) => sha,
        Err(e) => return Poll::Failed(e),
    };
    match known {
        None => Poll::First(to),
        Some(k) if k == to => Poll::Same,
        Some(k) => {
            if let Err(e) = fetch(repo, remote, branch) {
                return Poll::Failed(format!("{to} vu, mais git fetch a échoué : {e}"));
            }
            let log = log(repo, k, &to, 50);
            Poll::Moved { from: k.to_string(), to, log }
        }
    }
}
