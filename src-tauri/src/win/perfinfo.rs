//! Process/thread counts, commit charge and cache size from GetPerformanceInfo.

use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};

#[derive(Default)]
pub struct PerfInfo {
    pub processes: u32,
    pub threads: u32,
    pub committed: u64,
    pub commit_limit: u64,
    pub cached: u64,
}

pub fn read() -> PerfInfo {
    let mut pi = PERFORMANCE_INFORMATION { cb: std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32, ..Default::default() };
    if unsafe { GetPerformanceInfo(&mut pi, pi.cb) }.is_err() {
        return PerfInfo::default();
    }
    let page = pi.PageSize as u64;
    PerfInfo {
        processes: pi.ProcessCount,
        threads: pi.ThreadCount,
        committed: pi.CommitTotal as u64 * page,
        commit_limit: pi.CommitLimit as u64 * page,
        cached: pi.SystemCache as u64 * page,
    }
}

/// Private working set of one process (what Windows Task Manager shows as "Memory"): pages
/// only this process uses, so shared DLL pages aren't counted once per process.
pub fn private_working_set(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut c = PROCESS_MEMORY_COUNTERS_EX2 { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
        let r = GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        let _ = CloseHandle(h);
        r.ok()?;
        (c.PrivateWorkingSetSize > 0).then_some(c.PrivateWorkingSetSize as u64)
    }
}
