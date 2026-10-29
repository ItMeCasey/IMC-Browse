//! Process containment. WebView2 already runs renderers, GPU and utility
//! processes in Chromium's AppContainer/restricted-token sandbox, one renderer
//! group per site. On top of that we put the browser and everything it spawns
//! in a Job Object so no child survives us and none can create a breakaway.

use windows::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};

pub fn contain_process_tree() {
    unsafe {
        let Ok(job) = CreateJobObjectW(None, None) else { return };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // Kill every child when the browser exits; forbid escaping the job.
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .is_err()
        {
            return;
        }
        // The handle is deliberately never closed: closing it would kill the whole tree.
        let _ = AssignProcessToJobObject(job, GetCurrentProcess());
    }
}

/// A Windows system program by full path (never found through the current folder or PATH).
pub fn system_exe(name: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    if name.eq_ignore_ascii_case("explorer.exe") { root.join(name) } else { root.join("System32").join(name) }
}

/// Makes DLL planting harder: libraries load only from the system folder and the app's own, never from
/// the current folder or a network share, and never from low-integrity locations.
pub fn harden_process() {
    use windows::Win32::System::{
        LibraryLoader::{LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories, SetDllDirectoryW},
        Threading::{ProcessImageLoadPolicy, SetProcessMitigationPolicy},
    };
    unsafe {
        let _ = SetDllDirectoryW(windows::core::w!(""));
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
        // bit 0: NoRemoteImages, bit 1: NoLowMandatoryLabelImages, bit 2: PreferSystem32Images
        let flags: u32 = 0b111; // the policy structure is one DWORD of flags
        let _ = SetProcessMitigationPolicy(ProcessImageLoadPolicy, &flags as *const u32 as *const _, std::mem::size_of::<u32>());
    }
}
