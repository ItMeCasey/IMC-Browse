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
