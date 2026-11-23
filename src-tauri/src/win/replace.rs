//! "Open this instead of Windows Task Manager": the Image File Execution Options "Debugger"
//! redirect Process Explorer uses. Windows then starts our exe whenever taskmgr.exe is
//! launched (Ctrl+Shift+Esc, taskbar menu). Our exe ignores the extra taskmgr path argument.
//!
//! The installer hooks (installer-hooks.nsh) carry the setting across an "uninstall, then install
//! the new version" upgrade.

use serde::Serialize;
use winreg::enums::*;
use winreg::RegKey;

const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";
const EXE_NAME: &str = "taskmanager-remake.exe";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "state", content = "path")]
pub enum State {
    /// Ctrl+Shift+Esc opens this app.
    On,
    Off,
    /// Another program (e.g. Process Explorer) is set; we leave it alone unless the user
    /// switches this on.
    Other(String),
}

fn our_exe() -> Result<String, String> {
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).map_err(|e| e.to_string())
}

fn debugger() -> Option<String> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(IFEO).ok()?;
    let v: String = k.get_value("Debugger").ok()?;
    let v = v.trim().trim_matches('"').to_string();
    (!v.is_empty()).then_some(v)
}

fn is_our_name(path: &str) -> bool {
    path.rsplit('\\').next().is_some_and(|f| f.eq_ignore_ascii_case(EXE_NAME))
}

pub fn state() -> State {
    match debugger() {
        None => State::Off,
        Some(v) => match our_exe() {
            Ok(me) if v.eq_ignore_ascii_case(&me) => State::On,
            _ => State::Other(v),
        },
    }
}

pub fn set_enabled(on: bool) -> Result<State, String> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let err = |e: std::io::Error| format!("Could not change the setting: {e}");
    if on {
        let (k, _) = hklm.create_subkey(IFEO).map_err(err)?;
        k.set_value("Debugger", &format!("\"{}\"", our_exe()?)).map_err(err)?;
    } else if state() == State::On {
        // Only ever remove our own redirect, never another program's.
        let k = hklm.open_subkey_with_flags(IFEO, KEY_ALL_ACCESS).map_err(err)?;
        match k.delete_value("Debugger") {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
    }
    Ok(state())
}

/// Run at start-up: if the redirect points at a copy of this app that no longer exists (the app
/// was moved or reinstalled elsewhere), point it at this exe so Ctrl+Shift+Esc keeps working.
/// A redirect that is ours by name but missing would otherwise leave no task manager at all.
pub fn repair() {
    if let Some(v) = debugger() {
        if is_our_name(&v) && !std::path::Path::new(&v).exists() {
            let _ = set_enabled(true);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn our_name() {
        assert!(super::is_our_name(r"C:\Program Files\Task Manager Remake\taskmanager-remake.exe"));
        assert!(super::is_our_name(r"D:\x\TASKMANAGER-REMAKE.EXE"));
        assert!(!super::is_our_name(r"C:\Tools\procexp64.exe"));
    }
}
