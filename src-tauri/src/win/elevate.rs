//! The app always runs as administrator, but its manifest says "asInvoker": a launch that is
//! not elevated relaunches itself through the UAC prompt. A requireAdministrator manifest would
//! break the Ctrl+Shift+Esc redirect, because Windows starts the redirect target with
//! CreateProcess, which fails with ERROR_ELEVATION_REQUIRED instead of showing UAC.

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOWNORMAL,
};

pub fn is_elevated() -> bool {
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        let _ = CloseHandle(tok);
        ok.is_ok() && e.TokenIsElevated != 0
    }
}

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

struct Search {
    me: String,
    my_pid: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn each_window(h: HWND, l: LPARAM) -> BOOL {
    let s = unsafe { &mut *(l.0 as *mut Search) };
    unsafe {
        if !IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() {
            return BOOL(1);
        }
        let mut title = [0u16; 64];
        let n = GetWindowTextW(h, &mut title);
        if String::from_utf16_lossy(&title[..n.max(0) as usize]) != "Task Manager" {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != s.my_pid && process_path(pid).is_some_and(|p| p.eq_ignore_ascii_case(&s.me)) {
            s.found = Some(h);
            return BOOL(0);
        }
    }
    BOOL(1)
}

enum Found {
    None,
    Focused,
    /// Our window exists but stayed minimised (Windows can block restoring an admin window
    /// from a non-admin process).
    Minimised,
}

/// If this app is already open, bring its window to the front.
fn focus_existing() -> Found {
    let Ok(me) = std::env::current_exe() else { return Found::None };
    let mut s = Search { me: me.to_string_lossy().to_string(), my_pid: std::process::id(), found: None };
    unsafe {
        let _ = EnumWindows(Some(each_window), LPARAM(&mut s as *mut Search as isize));
        let Some(h) = s.found else { return Found::None };
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
        // Windows may refuse to bring it forward (it then flashes in the taskbar), which is fine.
        // A window that stayed minimised is not fine: let the elevated relaunch restore it.
        if IsIconic(h).as_bool() {
            Found::Minimised
        } else {
            Found::Focused
        }
    }
}

pub enum Launch {
    /// The open window was brought forward, or an elevated copy was started: exit.
    Handled,
    /// UAC was declined or failed: run without admin rights rather than open nothing (with the
    /// Ctrl+Shift+Esc redirect on, there would otherwise be no task manager at all).
    RunLimited,
}

/// Called when not elevated: focus the open window, or relaunch through UAC.
pub fn relaunch_elevated() -> Launch {
    let found = focus_existing();
    if matches!(found, Found::Focused) {
        return Launch::Handled;
    }
    // With a window already open, a failed UAC must not start a second, limited copy.
    let fallback = if matches!(found, Found::Minimised) { Launch::Handled } else { Launch::RunLimited };
    if run_as_admin(None) {
        Launch::Handled
    } else {
        fallback
    }
}

/// Starts this exe through the UAC prompt. `wait_for` makes the new copy wait until that process
/// (the limited copy asking to restart) has exited, so the two never meet in the single-instance
/// check. True when the elevated copy was started.
pub fn run_as_admin(wait_for: Option<u32>) -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    let params: Vec<u16> = wait_for.map(|p| format!("--wait-pid {p}")).unwrap_or_default().encode_utf16().chain(Some(0)).collect();
    let file: Vec<u16> = me.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.is_ok()
}

/// `--wait-pid N` on the command line: wait (up to 10 s) for process N to exit.
pub fn wait_for_previous() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args.iter().position(|a| a == "--wait-pid").and_then(|i| args.get(i + 1)).and_then(|p| p.parse::<u32>().ok()) else { return };
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
}
