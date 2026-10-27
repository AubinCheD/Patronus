//! Écritures atomiques et dossier de données.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// `%APPDATA%\Patronus`, ou `PATRONUS_DATA_DIR` s'il est défini (essais, profils séparés).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PATRONUS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    default_data_dir()
}

/// `%APPDATA%Patronus`, le dossier de données habituel.
pub fn default_data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Patronus")
}

/// Un programme de Windows par son chemin (`rel` relatif au dossier de Windows : `System32\cmd.exe`,
/// `explorer.exe`…). Appelé par son seul nom, Windows le chercherait d'abord dans le dossier de
/// l'exécutable de Patronus, où un `cmd.exe` déposé passerait avant celui du système.
pub fn windows_program(rel: &str) -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join(rel)
}

/// Écrit dans un fichier temporaire voisin puis le renomme : un arrêt brutal laisse l'ancien
/// contenu ou le nouveau, jamais un fichier tronqué.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Lit un fichier texte en acceptant la marque d'ordre d'octets UTF-8 (Bloc-notes l'ajoute).
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let s = fs::read_to_string(path)?;
    Ok(s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s))
}

/// Dossier temporaire propre à un test, vidé à la création.
#[cfg(test)]
pub fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("patronus-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Les programmes du système que Patronus lance sont trouvés à leur place, sans recherche.
    #[test]
    fn system_programs_are_found_at_their_place() {
        for rel in [
            r"System32\cmd.exe",
            r"System32\WindowsPowerShell\v1.0\powershell.exe",
            r"System32\schtasks.exe",
            r"System32\taskkill.exe",
            "explorer.exe",
        ] {
            let p = windows_program(rel);
            assert!(p.is_absolute() && p.is_file(), "{}", p.display());
        }
    }
}
