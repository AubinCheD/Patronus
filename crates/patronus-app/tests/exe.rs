//! L'exécutable porte ses informations de version (Propriétés > Détails, Gestionnaire des tâches)
//! et son icône (Explorateur, barre des tâches) : ressources écrites par `build.rs`.

use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::UI::Shell::ExtractIconExW;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_patronus"))
}

/// Une chaîne de la table française (040C, Unicode) des informations de version.
fn version_string(path: &Path, key: &str) -> Option<String> {
    let p = wide(&path.to_string_lossy());
    unsafe {
        let size = GetFileVersionInfoSizeW(p.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(p.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let q = wide(&format!(r"\StringFileInfo\040C04B0\{key}"));
        let (mut value, mut len) = (std::ptr::null_mut(), 0u32);
        if VerQueryValueW(buf.as_ptr().cast(), q.as_ptr(), &mut value, &mut len) == 0 || len == 0 {
            return None;
        }
        let units = std::slice::from_raw_parts(value as *const u16, len as usize);
        Some(String::from_utf16_lossy(units).trim_end_matches('\0').to_string())
    }
}

#[test]
fn exe_has_version_info() {
    assert_eq!(version_string(exe(), "ProductName").as_deref(), Some("Patronus"));
    assert_eq!(version_string(exe(), "FileVersion").as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(version_string(exe(), "OriginalFilename").as_deref(), Some("patronus.exe"));
}

#[test]
fn exe_has_icon() {
    let p = wide(&exe().to_string_lossy());
    // Index -1 : le nombre d'icônes du fichier.
    let count = unsafe { ExtractIconExW(p.as_ptr(), -1, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
    assert_eq!(count, 1, "un groupe d'icônes");
}
