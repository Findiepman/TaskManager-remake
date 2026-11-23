//! Startup impact as Windows measures it. After each sign-in Windows writes
//! System32\wdi\LogFiles\StartupInfo\<user SID>_StartupInfo<N>.xml with the CPU time and
//! disk bytes each startup process used. Task Manager reads the same files (admin only).

use crate::logic;
use std::collections::HashMap;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn current_sid() -> Option<String> {
    unsafe {
        let mut tok = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(tok, TokenUser, Some(buf.as_mut_ptr() as _), len, &mut len);
        let _ = CloseHandle(tok);
        ok.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        out
    }
}

/// Normalised exe path -> (cpu microseconds, disk bytes) from the most recent sign-in.
/// Empty when the files are missing or unreadable; the UI then says "Not measured".
pub fn impacts() -> HashMap<String, (u64, u64)> {
    let mut out = HashMap::new();
    let Some(sid) = current_sid() else { return out };
    let dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()) + r"\System32\wdi\LogFiles\StartupInfo";
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let prefix = format!("{}_startupinfo", sid.to_lowercase());
    let newest = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(&prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t);
    let Some((_, path)) = newest else { return out };
    let Ok(bytes) = std::fs::read(&path) else { return out };
    let text = decode(&bytes);
    for (name, cpu, disk) in logic::parse_startup_info(&text) {
        let e = out.entry(logic::norm_path(&name)).or_insert((0, 0));
        e.0 += cpu;
        e.1 += disk;
    }
    out
}

/// The files are usually UTF-16 LE with a BOM; accept UTF-8 too.
fn decode(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFF && b[1] == 0xFE {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)).to_string()
    }
}
