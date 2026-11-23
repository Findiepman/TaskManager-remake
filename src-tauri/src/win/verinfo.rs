//! Reads FileDescription and CompanyName from an exe's version resource.

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};

#[derive(Clone, Default, Debug)]
pub struct VerInfo {
    pub description: Option<String>,
    pub company: Option<String>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn read(path: &str) -> VerInfo {
    let mut out = VerInfo::default();
    let wpath = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return out;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), None, size, buf.as_mut_ptr() as _).is_err() {
            return out;
        }
        // Use the first language/codepage pair the file declares, then the common fallbacks.
        let mut langs: Vec<(u16, u16)> = Vec::new();
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let tr = wide(r"\VarFileInfo\Translation");
        if VerQueryValueW(buf.as_ptr() as _, PCWSTR(tr.as_ptr()), &mut p, &mut len).as_bool() && len >= 4 {
            let pairs = std::slice::from_raw_parts(p as *const u16, (len / 2) as usize);
            for c in pairs.chunks_exact(2) {
                langs.push((c[0], c[1]));
            }
        }
        langs.extend([(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0000, 0x04B0)]);
        let query = |field: &str| -> Option<String> {
            for (l, c) in &langs {
                let key = wide(&format!(r"\StringFileInfo\{:04x}{:04x}\{}", l, c, field));
                let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if VerQueryValueW(buf.as_ptr() as _, PCWSTR(key.as_ptr()), &mut p, &mut len).as_bool() && len > 0 {
                    // Some exes report a length past the string; stop at the first NUL.
                    let s = std::slice::from_raw_parts(p as *const u16, len as usize);
                    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                    let s = String::from_utf16_lossy(&s[..end]).trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            None
        };
        out.description = query("FileDescription");
        out.company = query("CompanyName");
    }
    out
}
