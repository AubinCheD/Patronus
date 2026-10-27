//! L'exécutable que Patronus inscrit (Planificateur de tâches, démarrage de Windows) doit être à l'abri
//! des autres comptes du poste : s'ils peuvent le remplacer, ou déposer à côté une DLL ou un `cmd.exe`
//! que Windows prendrait avant ceux du système, leur code s'exécuterait sous le compte de
//! l'utilisateur à chaque échéance. C'est le cas d'un dossier créé à la racine d'un disque
//! (`C:\Outils`), qui hérite d'un droit de modification pour tous les utilisateurs authentifiés ;
//! `%LOCALAPPDATA%\Programs` et `Program Files` sont sûrs.

use std::path::Path;

/// Rend une erreur, en français, si l'exécutable `exe` ou son dossier peut être modifié par un autre
/// compte que l'utilisateur, SYSTEM, les administrateurs ou TrustedInstaller.
pub fn check(exe: &Path) -> Result<(), String> {
    let dir = exe.parent().unwrap_or(exe);
    for path in [exe, dir] {
        if let Some(who) = writable_by_others(path)? {
            return Err(format!(
                "{} peut être modifié par un autre compte du poste ({who}) : une tâche ou un démarrage \
                 automatique lancerait ce qu'il y dépose. Installez l'application dans \
                 %LOCALAPPDATA%\\Programs ou dans Program Files.",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Premier compte (SID) autre que l'utilisateur et le système qui a un droit d'écriture sur `path`.
#[cfg(windows)]
pub fn writable_by_others(path: &Path) -> Result<Option<String>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE, PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;

    /// Écrire, ajouter un fichier ou un sous-dossier, supprimer, changer les droits ou le propriétaire.
    const WRITE: u32 = 0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
    const TRUSTED: [&str; 5] = [
        "S-1-5-18",                                                       // SYSTEM
        "S-1-5-32-544",                                                   // Administrateurs
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
        "S-1-3-0", // CREATOR OWNER (le créateur d'un fichier, donc l'utilisateur ici)
        "S-1-3-4", // OWNER RIGHTS
    ];
    let me = current_user_sid().ok_or("compte de l'utilisateur illisible")?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY : chemin terminé par un zéro ; `sd` est libéré par LocalFree, `dacl` pointe dedans.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "droits de {} illisibles : {}",
            path.display(),
            std::io::Error::from_raw_os_error(status as i32)
        ));
    }
    let mut found = None;
    if dacl.is_null() {
        // Pas de liste de contrôle : tout le monde a tous les droits.
        found = Some("tout le monde".to_string());
    } else {
        // SAFETY : `dacl` est valide tant que `sd` n'est pas libéré ; chaque ACE rendue par GetAce aussi.
        let count = unsafe { (*dacl).AceCount };
        for i in 0..u32::from(count) {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            if unsafe { GetAce(dacl, i, &mut ace) } == 0 || ace.is_null() {
                continue;
            }
            let ace = ace as *const ACCESS_ALLOWED_ACE;
            let header = unsafe { (*ace).Header };
            if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
                || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
                || unsafe { (*ace).Mask } & WRITE == 0
            {
                continue;
            }
            let sid = unsafe { sid_string(std::ptr::addr_of!((*ace).SidStart) as *mut core::ffi::c_void) };
            if let Some(sid) = sid
                && sid != me
                && !TRUSTED.contains(&sid.as_str())
            {
                found = Some(sid);
                break;
            }
        }
    }
    unsafe { LocalFree(sd) };
    Ok(found)
}

#[cfg(not(windows))]
pub fn writable_by_others(_path: &Path) -> Result<Option<String>, String> {
    Ok(None)
}

/// Forme texte (`S-1-5-…`) d'un SID.
#[cfg(windows)]
unsafe fn sid_string(sid: windows_sys::Win32::Security::PSID) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY : `sid` valide (appelant) ; le texte rendu est libéré par LocalFree.
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text) == 0 || text.is_null() {
            return None;
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Some(s)
    }
}

/// SID de l'utilisateur du processus.
#[cfg(windows)]
fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY : jeton fermé avant de sortir ; tampon aligné pour TOKEN_USER, de la taille demandée.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        sid_string((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn icacls(path: &Path, args: &[&str]) {
        let ok = std::process::Command::new("icacls").arg(path).args(args).output().unwrap().status.success();
        assert!(ok, "icacls {args:?}");
    }

    /// Un dossier du profil est à l'abri ; ouvert en écriture à tous, il ne l'est plus, ni l'exécutable qu'il
    /// contient.
    #[test]
    fn a_folder_others_can_write_to_is_refused() {
        let dir = std::env::temp_dir().join(format!("patronus-exeplace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("patronus.exe");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(writable_by_others(&dir), Ok(None));
        assert_eq!(check(&exe), Ok(()));
        icacls(&dir, &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        assert_eq!(writable_by_others(&dir), Ok(Some("S-1-5-11".into())));
        let err = check(&exe).unwrap_err();
        assert!(err.contains("S-1-5-11") && err.contains("Program Files"), "{err}");
        icacls(&dir, &["/remove:g", "*S-1-5-11"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
