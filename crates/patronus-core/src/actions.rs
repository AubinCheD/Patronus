//! Actions sur un résultat, toujours à la demande de l'utilisateur (jamais automatiques) : appliquer
//! le diff d'une session dans la copie de travail, reprendre la session Claude dans un terminal.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
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

/// Ce que l'application du diff toucherait : la branche courante du dépôt et les fichiers du diff.
/// Erreur si le diff ne s'applique pas proprement (copie de travail qui a divergé).
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyPlan {
    pub branch: String,
    pub files: Vec<String>,
}

pub fn check_apply(repo: &Path, patch: &Path) -> Result<ApplyPlan, String> {
    let patch_arg = patch.to_string_lossy().into_owned();
    git(repo, &["apply", "--check", &patch_arg])
        .map_err(|e| format!("le diff ne s'applique pas à la copie de travail actuelle : {e}"))?;
    let numstat = git(repo, &["apply", "--numstat", &patch_arg])?;
    let files = numstat.lines().filter_map(|l| l.splitn(3, '\t').nth(2)).map(str::to_string).collect();
    let branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|b| b.trim().to_string()).unwrap_or_default();
    Ok(ApplyPlan { branch, files })
}

/// Applique le diff dans la copie de travail seulement : ni index, ni commit.
pub fn apply(repo: &Path, patch: &Path) -> Result<(), String> {
    git(repo, &["apply", &patch.to_string_lossy()]).map(|_| ())
}

/// Un programme à lancer, décrit avant d'être lancé (testable sans rien lancer).
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    /// Une console à part (PowerShell) ; Windows Terminal ouvre sa propre fenêtre.
    pub new_console: bool,
}

impl Launch {
    pub fn spawn(&self) -> std::io::Result<()> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).current_dir(&self.current_dir);
        #[cfg(windows)]
        if self.new_console {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0010);
        }
        cmd.spawn().map(|_| ())
    }
}

/// Windows Terminal installé pour l'utilisateur, ou dans le `PATH`.
pub fn windows_terminal() -> Option<PathBuf> {
    dirs::data_local_dir()
        .map(|d| d.join("Microsoft").join("WindowsApps").join("wt.exe"))
        .filter(|p| p.is_file())
        .or_else(|| std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("wt.exe")).find(|p| p.is_file()))
}

/// Reprendre une session Claude dans son dossier : `claude --resume <session>`, dans Windows Terminal
/// s'il est là (`wt`), sinon dans une console PowerShell.
pub fn resume_command(dir: &Path, session: &str, claude: &Path, wt: Option<&Path>) -> Launch {
    match wt {
        Some(wt) => Launch {
            program: wt.as_os_str().to_owned(),
            // Windows Terminal lit `;` comme un séparateur de commandes : il s'écrit `\;` dans un chemin.
            args: vec![
                "-d".into(),
                dir.to_string_lossy().replace(';', "\\;").into(),
                claude.as_os_str().to_owned(),
                "--resume".into(),
                session.into(),
            ],
            current_dir: dir.to_path_buf(),
            new_console: false,
        },
        None => Launch {
            program: "powershell.exe".into(),
            args: vec![
                "-NoExit".into(),
                "-Command".into(),
                format!(
                    "& '{}' --resume '{}'",
                    claude.display().to_string().replace('\'', "''"),
                    session.replace('\'', "''")
                )
                .into(),
            ],
            current_dir: dir.to_path_buf(),
            new_console: true,
        },
    }
}
