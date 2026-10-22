//! Notification Windows à la fin d'une exécution : une icône dans la zone de notification, ajoutée
//! à la première notification, et sa bulle, que Windows affiche comme une notification.

use std::sync::Mutex;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HICON, IDI_APPLICATION, IMAGE_ICON, LoadIconW, LoadImageW, SM_CXSMICON, SM_CYSMICON,
};

const ICON_ID: u32 = 1;

/// Fenêtre propriétaire de l'icône, et si l'icône est déjà dans la zone.
static TRAY: Mutex<(isize, bool)> = Mutex::new((0, false));

/// À appeler une fois, avec la fenêtre de l'application.
pub fn init(hwnd: isize) {
    TRAY.lock().unwrap().0 = hwnd;
}

fn copy(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n).chain(std::iter::once(0))) {
        *d = c;
    }
}

fn data(hwnd: isize) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd as _;
    nid.uID = ICON_ID;
    nid
}

/// L'icône de l'exécutable (ressource n° 1, voir `build.rs`) en petite taille ; à défaut, celle
/// des applications.
fn tray_icon() -> HICON {
    unsafe {
        let (w, h) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let icon = LoadImageW(GetModuleHandleW(std::ptr::null()), 1 as _, IMAGE_ICON, w, h, 0);
        if icon.is_null() { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) } else { icon as HICON }
    }
}

/// Affiche une notification (sans effet si la fenêtre est inconnue).
pub fn notify(title: &str, body: &str) {
    let mut tray = TRAY.lock().unwrap();
    if tray.0 == 0 {
        return;
    }
    unsafe {
        if !tray.1 {
            let mut nid = data(tray.0);
            nid.uFlags = NIF_ICON | NIF_TIP;
            nid.hIcon = tray_icon();
            copy(&mut nid.szTip, "Patronus");
            tray.1 = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
        }
        let mut nid = data(tray.0);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        copy(&mut nid.szInfoTitle, title);
        copy(&mut nid.szInfo, body);
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Retire l'icône de la zone de notification (à la fermeture).
pub fn cleanup() {
    let mut tray = TRAY.lock().unwrap();
    if tray.1 {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data(tray.0));
        }
        tray.1 = false;
    }
}
