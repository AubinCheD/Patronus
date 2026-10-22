//! Lancement de Patronus à l'ouverture de session Windows : une valeur de la clé
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, sans droit administrateur, fenêtre réduite.
//! Désactivé par défaut (un programme non signé qui s'inscrit au démarrage est un comportement que
//! les antivirus notent). Même procédé que Fawkes.
//!
//! Le nom de la valeur est celui du dossier du Planificateur (`Patronus`, ou `Patronus-<empreinte>`
//! pour un autre dossier de données) : une instance d'essai ne remplace pas celle de tous les jours.

use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Où le Gestionnaire des tâches note les programmes de démarrage que l'utilisateur a désactivés.
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    /// Inscrit pour cet exécutable et ce dossier de données.
    On,
    /// Inscrit avec une autre commande (exécutable déplacé, autre version) : la commande inscrite.
    OtherCommand(String),
    /// Inscrit, mais désactivé dans le Gestionnaire des tâches.
    DisabledByUser,
}

/// Où l'inscription d'un dossier de données est rangée dans le registre.
struct Place {
    run: String,
    approved: String,
    name: String,
}

impl Place {
    fn real(data_dir: &Path) -> Place {
        Place {
            run: RUN_KEY.into(),
            approved: APPROVED_KEY.into(),
            name: crate::winsched::folder(data_dir, &crate::fsutil::default_data_dir()),
        }
    }
}

pub fn status(data_dir: &Path) -> Status {
    status_in(&Place::real(data_dir), &command_for(&current_exe(), data_dir))
}

pub fn set(data_dir: &Path, on: bool) -> Result<(), String> {
    set_in(&Place::real(data_dir), on, &command_for(&current_exe(), data_dir))
}

fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("patronus.exe"))
}

/// `"<exe>" [--donnees "<dossier>"] --reduit`.
fn command_for(exe: &Path, data_dir: &Path) -> String {
    let default = crate::fsutil::default_data_dir();
    if crate::winsched::folder(data_dir, &default) == "Patronus" {
        format!("\"{}\" --reduit", exe.display())
    } else {
        // Sans barre oblique finale : `\"` échapperait le guillemet fermant.
        let dir = data_dir.to_string_lossy().replace('/', "\\");
        format!("\"{}\" --donnees \"{}\" --reduit", exe.display(), dir.trim_end_matches('\\'))
    }
}

fn status_in(p: &Place, command: &str) -> Status {
    let Some(cmd) = read_string(&p.run, &p.name) else { return Status::Off };
    // 02… : activé ; 03… : désactivé dans le Gestionnaire des tâches (premier octet impair).
    if read_value(&p.approved, &p.name, RRF_RT_REG_BINARY).and_then(|b| b.first().copied()).is_some_and(|b| b & 1 == 1)
    {
        return Status::DisabledByUser;
    }
    if cmd.eq_ignore_ascii_case(command) { Status::On } else { Status::OtherCommand(cmd) }
}

fn set_in(p: &Place, on: bool, command: &str) -> Result<(), String> {
    // Recocher l'option annule une désactivation faite dans le Gestionnaire des tâches.
    delete_value(&p.approved, &p.name)?;
    if on {
        let data: Vec<u8> = command.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        write_value(&p.run, &p.name, REG_SZ, &data)
    } else {
        delete_value(&p.run, &p.name)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn os_error(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// Valeur de `HKCU\<key>`, ou `None` si elle n'existe pas.
fn read_value(key: &str, name: &str, flags: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let (k, n) = (wide(key), wide(name));
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), flags, null_mut(), null_mut(), &mut size)
            != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            flags,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

fn read_string(key: &str, name: &str) -> Option<String> {
    let bytes = read_value(key, name, RRF_RT_REG_SZ)?;
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
}

/// Écrit une valeur (la clé est créée au besoin).
fn write_value(key: &str, name: &str, kind: u32, data: &[u8]) -> Result<(), String> {
    let (k, n) = (wide(key), wide(name));
    let r = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), kind, data.as_ptr().cast(), data.len() as u32)
    };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(os_error(r)) }
}

/// Supprime une valeur ; absente, il n'y a rien à faire.
fn delete_value(key: &str, name: &str) -> Result<(), String> {
    let (k, n) = (wide(key), wide(name));
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(os_error(r)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delete_tree(key: &str) {
        let k = wide(key);
        unsafe { windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
    }

    #[test]
    fn command_names_the_data_dir_unless_default() {
        let exe = Path::new(r"C:\Program Files\Patronus\patronus.exe");
        assert_eq!(
            command_for(exe, &crate::fsutil::default_data_dir()),
            r#""C:\Program Files\Patronus\patronus.exe" --reduit"#
        );
        for dir in [r"D:\essais\patronus", r"D:\essais\patronus\", "D:/essais/patronus/"] {
            assert_eq!(
                command_for(exe, Path::new(dir)),
                r#""C:\Program Files\Patronus\patronus.exe" --donnees "D:\essais\patronus" --reduit"#
            );
        }
    }

    /// Inscription, autre commande, désactivation par le Gestionnaire des tâches puis retrait : sur
    /// une clé de test, jamais sur la vraie clé `Run`.
    #[test]
    fn registers_and_unregisters() {
        let base = format!(r"Software\Patronus-essai-{}", std::process::id());
        let p = Place { run: format!(r"{base}\Run"), approved: format!(r"{base}\Approved"), name: "Patronus".into() };
        let cmd = r#""C:\Program Files\Patronus\patronus.exe" --reduit"#;
        assert_eq!(status_in(&p, cmd), Status::Off);
        set_in(&p, true, cmd).unwrap();
        assert_eq!(status_in(&p, cmd), Status::On);
        assert_eq!(read_string(&p.run, "Patronus").as_deref(), Some(cmd));
        assert_eq!(status_in(&p, r#""C:\autre\patronus.exe" --reduit"#), Status::OtherCommand(cmd.into()));
        // Le Gestionnaire des tâches désactive en écrivant 03 suivi d'une date.
        write_value(
            &p.approved,
            "Patronus",
            windows_sys::Win32::System::Registry::REG_BINARY,
            &[3, 0, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
        assert_eq!(status_in(&p, cmd), Status::DisabledByUser);
        // Recocher l'option la réactive.
        set_in(&p, true, cmd).unwrap();
        assert_eq!(status_in(&p, cmd), Status::On);
        set_in(&p, false, cmd).unwrap();
        assert_eq!(status_in(&p, cmd), Status::Off);
        delete_tree(&base);
    }
}
