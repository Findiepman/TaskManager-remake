//! Startup apps: the same sources and on/off switch Windows Task Manager uses.
//! Run keys and Startup folders list the items; Explorer\StartupApproved holds the switch.

use crate::logic;
use serde::Serialize;
use std::collections::HashMap;
use winreg::enums::*;
use winreg::RegKey;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Serialize, Clone, Debug)]
pub struct StartupItem {
    /// "<source>|<value name>", stable across refreshes; used to toggle.
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub command: String,
    pub enabled: bool,
    /// "High" / "Medium" / "Low", or None when Windows has not measured it.
    pub impact: Option<String>,
}

#[derive(Clone, Copy)]
enum Source {
    UserRun,
    MachineRun,
    MachineRun32,
    UserFolder,
    MachineFolder,
}

impl Source {
    fn tag(self) -> &'static str {
        match self {
            Source::UserRun => "hkcu-run",
            Source::MachineRun => "hklm-run",
            Source::MachineRun32 => "hklm-run32",
            Source::UserFolder => "user-folder",
            Source::MachineFolder => "machine-folder",
        }
    }
    fn from_tag(t: &str) -> Option<Self> {
        Some(match t {
            "hkcu-run" => Source::UserRun,
            "hklm-run" => Source::MachineRun,
            "hklm-run32" => Source::MachineRun32,
            "user-folder" => Source::UserFolder,
            "machine-folder" => Source::MachineFolder,
            _ => return None,
        })
    }
    /// (hive, subkey under StartupApproved)
    fn approved(self) -> (isize, &'static str) {
        match self {
            Source::UserRun => (HKEY_CURRENT_USER as isize, "Run"),
            Source::MachineRun => (HKEY_LOCAL_MACHINE as isize, "Run"),
            Source::MachineRun32 => (HKEY_LOCAL_MACHINE as isize, "Run32"),
            Source::UserFolder => (HKEY_CURRENT_USER as isize, "StartupFolder"),
            Source::MachineFolder => (HKEY_LOCAL_MACHINE as isize, "StartupFolder"),
        }
    }
}

fn approved_key(src: Source) -> (RegKey, String) {
    let (hive, sub) = src.approved();
    (RegKey::predef(hive as _), format!(r"{APPROVED}\{sub}"))
}

fn is_enabled(src: Source, value: &str) -> bool {
    let (root, path) = approved_key(src);
    let data = root.open_subkey(&path).ok().and_then(|k| k.get_raw_value(value).ok()).map(|v| v.bytes);
    logic::startup_approved_enabled(data.as_deref())
}

fn folder(src: Source) -> Option<std::path::PathBuf> {
    let base = match src {
        Source::UserFolder => std::env::var_os("APPDATA")?,
        Source::MachineFolder => std::env::var_os("ProgramData")?,
        _ => return None,
    };
    Some(std::path::PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

pub fn list() -> Vec<StartupItem> {
    let impacts = super::startupinfo::impacts();
    let mut out = Vec::new();
    for (src, hive, path) in [
        (Source::UserRun, HKEY_CURRENT_USER, RUN),
        (Source::MachineRun, HKEY_LOCAL_MACHINE, RUN),
        (Source::MachineRun32, HKEY_LOCAL_MACHINE, RUN32),
    ] {
        let Ok(key) = RegKey::predef(hive).open_subkey(path) else { continue };
        for (name, _) in key.enum_values().flatten() {
            let Ok(cmd) = key.get_value::<String, _>(&name) else { continue };
            if cmd.trim().is_empty() {
                continue;
            }
            let exe = logic::exe_from_command(&cmd).map(|e| expand_env(&e));
            out.push(make_item(src, &name, &cmd, exe.as_deref(), &impacts));
        }
    }
    for src in [Source::UserFolder, Source::MachineFolder] {
        let Some(dir) = folder(src) else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let full = e.path();
            let target = if file.to_lowercase().ends_with(".lnk") { super::shortcut::target(&full) } else { Some(full.to_string_lossy().to_string()) };
            let cmd = target.clone().unwrap_or_else(|| full.to_string_lossy().to_string());
            let mut item = make_item(src, &file, &cmd, target.as_deref(), &impacts);
            if item.name == file {
                item.name = file.trim_end_matches(".lnk").trim_end_matches(".LNK").to_string();
            }
            out.push(item);
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn make_item(src: Source, value: &str, cmd: &str, exe: Option<&str>, impacts: &HashMap<String, (u64, u64)>) -> StartupItem {
    let ver = exe.map(super::verinfo::read).unwrap_or_default();
    // A script or command run through a generic host (wscript, cmd, powershell...) would all be
    // called "Windows Based Script Host"; the entry's own name says what it actually is.
    let name = if exe.is_some_and(logic::is_generic_host) {
        value.to_string()
    } else {
        ver.description.clone().filter(|d| d.len() <= 60).unwrap_or_else(|| value.to_string())
    };
    let impact = exe
        .and_then(|e| impacts.get(&logic::norm_path(e)))
        .map(|&(cpu, disk)| logic::startup_impact(cpu, disk).to_string());
    StartupItem {
        id: format!("{}|{}", src.tag(), value),
        name,
        publisher: ver.company.unwrap_or_default(),
        command: cmd.to_string(),
        enabled: is_enabled(src, value),
        impact,
    }
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let var = &after[..j];
                match std::env::var(var) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let (tag, value) = id.split_once('|').ok_or("Unknown startup item.")?;
    let src = Source::from_tag(tag).ok_or("Unknown startup item.")?;
    let (root, path) = approved_key(src);
    let (key, _) = root.create_subkey(&path).map_err(|e| format!("Could not open the startup settings: {e}"))?;
    // Same 12-byte layout Task Manager writes: state byte, 3 zero bytes, FILETIME of the change.
    let mut data = vec![0u8; 12];
    data[0] = if enabled { 2 } else { 3 };
    if !enabled {
        let ft = filetime_now();
        data[4..12].copy_from_slice(&ft.to_le_bytes());
    }
    key.set_raw_value(value, &winreg::RegValue { bytes: data.into(), vtype: REG_BINARY })
        .map_err(|e| format!("Could not change the startup setting: {e}"))
}

fn filetime_now() -> u64 {
    // 100 ns intervals since 1601-01-01.
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (unix.as_nanos() / 100) as u64 + 116_444_736_000_000_000
}

#[cfg(test)]
mod tests {
    #[test]
    fn env_expansion() {
        std::env::set_var("TM_TEST_X", "C:\\X");
        assert_eq!(super::expand_env("%TM_TEST_X%\\a.exe"), "C:\\X\\a.exe");
        assert_eq!(super::expand_env("%NOPE_NOT_SET_42%\\a"), "%NOPE_NOT_SET_42%\\a");
        assert_eq!(super::expand_env("50% off"), "50% off");
    }
}
