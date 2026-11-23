//! Ending processes and opening their file location.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetProcessTimes, IsProcessCritical, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

/// Ending any of these crashes Windows (blue screen) or logs the user out. Windows' own
/// "critical" flag (IsProcessCritical) is checked as well; this list is the second line.
const CRITICAL: &[&str] = &[
    "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
    "lsass.exe", "lsaiso.exe", "memory compression", "secure system", "system idle process",
];

pub fn is_critical_name(name: &str) -> bool {
    CRITICAL.iter().any(|c| c.eq_ignore_ascii_case(name))
}

struct Proc(HANDLE);
impl Drop for Proc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open(pid: u32, terminate: bool) -> Option<Proc> {
    let access = if terminate { PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION } else { PROCESS_QUERY_LIMITED_INFORMATION };
    unsafe { OpenProcess(access, false, pid).ok().map(Proc) }
}

fn flagged_critical(p: &Proc) -> bool {
    let mut c = windows::core::BOOL(0);
    // If Windows can't tell us, assume critical: refusing is always safe.
    unsafe { IsProcessCritical(p.0, &mut c).map(|_| c.as_bool()).unwrap_or(true) }
}

fn image_file(p: &Proc) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(p.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()? };
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(path.rsplit('\\').next().unwrap_or(&path).to_string())
}

/// Process start time in seconds since the Unix epoch (same scale as sysinfo's start_time).
fn start_time(p: &Proc) -> Option<u64> {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(p.0, &mut c, &mut e, &mut k, &mut u).ok()? };
    let ft = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
    Some((ft / 10_000_000).saturating_sub(11_644_473_600))
}

/// Whether ending this process would crash Windows. Used to grey out the buttons; `kill`
/// checks again right before ending anything.
pub fn is_critical_pid(pid: u32, name: &str) -> bool {
    if pid == 0 || pid == 4 || is_critical_name(name) {
        return true;
    }
    match open(pid, false) {
        Some(p) => flagged_critical(&p),
        // Protected processes we can't even query can't be ended either.
        None => true,
    }
}

/// Ends one process. `expected_start` is the start time from the snapshot the user clicked on:
/// if the pid now belongs to a different (newer) process, nothing is ended.
pub fn kill(pid: u32, expected_start: Option<u64>) -> Result<(), String> {
    if pid == 0 || pid == 4 || pid == std::process::id() {
        return Err("Windows needs this process to keep running.".into());
    }
    let p = open(pid, true).ok_or_else(|| format!("Could not end process {pid}: access denied or it already ended."))?;
    let name = image_file(&p).unwrap_or_default();
    if flagged_critical(&p) || is_critical_name(&name) {
        return Err(format!("{} is a critical Windows process. Ending it would crash Windows.", if name.is_empty() { format!("Process {pid}") } else { name }));
    }
    if let Some(want) = expected_start {
        // Can't confirm it is the same process: refuse rather than risk ending another one.
        match start_time(&p) {
            Some(now) if want.abs_diff(now) <= 2 => {}
            _ => return Err(format!("Process {pid} already ended.")),
        }
    }
    unsafe { TerminateProcess(p.0, 1) }.map_err(|e| format!("Could not end {}: {}", if name.is_empty() { format!("process {pid}") } else { name }, e.message()))
}

pub fn open_location(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let p = std::path::Path::new(path);
    // Only local drive paths (C:\...): a UNC path would make Explorer contact another machine.
    let b = path.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' || path.contains('"') {
        return Err("Can't open that location.".into());
    }
    if !p.exists() {
        return Err("That file no longer exists.".into());
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn critical_names() {
        assert!(super::is_critical_name("CSRSS.EXE"));
        assert!(super::is_critical_name("Memory Compression"));
        assert!(!super::is_critical_name("explorer.exe"));
    }

    #[test]
    fn own_process_is_not_critical_and_idle_is() {
        assert!(!super::is_critical_pid(std::process::id(), "test.exe"));
        assert!(super::is_critical_pid(0, "System Idle Process"));
        assert!(super::is_critical_pid(4, "System"));
    }
}
