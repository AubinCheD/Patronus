//! Processus Windows : objet de tâche qui tient un processus et toute sa descendance, et état d'un
//! processus d'après son numéro (vivant ? créé quand ?).

use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread,
    THREAD_SUSPEND_RESUME,
};

/// Objet de tâche : le processus qu'on y range et tout ce qu'il lance y sont tenus ; le fermer (ou
/// abandonner la valeur) les arrête tous, même si Patronus s'arrête brutalement.
pub struct Job(HANDLE);

// Le handle d'un objet de tâche peut servir depuis n'importe quel fil.
unsafe impl Send for Job {}

impl Job {
    pub fn new() -> Option<Job> {
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(Job(h))
        }
    }

    /// Range le processus dans l'objet.
    pub fn assign(&self, child: &std::process::Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) != 0 }
    }

    /// Arrête tous les processus de l'objet.
    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }

    /// Processus encore vivants dans l'objet.
    pub fn active_processes(&self) -> u32 {
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 { 0 } else { info.ActiveProcesses }
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Relance les fils d'un processus créé suspendu (`CREATE_SUSPENDED`). Rend le nombre de fils
/// relancés ; réessaie un court moment si l'instantané n'en montre encore aucun.
pub fn resume_threads(pid: u32) -> usize {
    for _ in 0..20 {
        let n = resume_once(pid);
        if n > 0 {
            return n;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    0
}

fn resume_once(pid: u32) -> usize {
    let mut resumed = 0;
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap.is_null() || snap as isize == -1 {
            return 0;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !t.is_null() {
                    // Nombre de suspensions avant l'appel : plus de zéro, le fil était bien suspendu.
                    if ResumeThread(t) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    resumed
}

/// Heure de création du processus `pid` (FILETIME brut), s'il existe encore et qu'on peut le lire.
pub fn created(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0;
        CloseHandle(h);
        (alive && ok).then_some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

/// Le processus `pid` tourne-t-il encore, et est-ce bien celui créé à `at` (le numéro d'un processus
/// terminé peut être redonné à un autre) ?
pub fn is_alive(pid: u32, at: u64) -> bool {
    created(pid) == Some(at)
}

/// Numéro et heure de création du processus courant.
pub fn current() -> (u32, u64) {
    let pid = std::process::id();
    (pid, created(pid).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive_and_an_old_number_is_not() {
        let (pid, at) = current();
        assert!(at > 0);
        assert!(is_alive(pid, at));
        assert!(!is_alive(pid, at + 1), "même numéro, autre heure de création : un autre processus");
        let mut child = std::process::Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
        let cpid = child.id();
        let cat = created(cpid);
        child.wait().unwrap();
        assert!(cat.is_none_or(|at| !is_alive(cpid, at)), "processus terminé");
    }
}
