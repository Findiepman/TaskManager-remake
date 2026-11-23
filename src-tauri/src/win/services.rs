//! Windows services: list, start, stop.

use serde::Serialize;
use windows::core::PCWSTR;
use windows::Win32::System::Services::*;

#[derive(Serialize, Clone, Debug)]
pub struct Service {
    /// Internal service name, used for start/stop.
    pub id: String,
    pub name: String,
    pub description: String,
    pub running: bool,
    /// "Automatic", "Manual", "Disabled", "Boot", "System"
    pub start_type: String,
    pub can_stop: bool,
}

struct Scm(SC_HANDLE);
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn manager(access: u32) -> Result<Scm, String> {
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access).map(Scm).map_err(|e| e.message().to_string()) }
}

fn start_type_name(t: SERVICE_START_TYPE) -> &'static str {
    match t {
        SERVICE_AUTO_START => "Automatic",
        SERVICE_DEMAND_START => "Manual",
        SERVICE_DISABLED => "Disabled",
        SERVICE_BOOT_START => "Boot",
        SERVICE_SYSTEM_START => "System",
        _ => "Manual",
    }
}

fn details(scm: &Scm, id: &str) -> (String, String) {
    unsafe {
        let w = wide(id);
        let Ok(h) = OpenServiceW(scm.0, PCWSTR(w.as_ptr()), SERVICE_QUERY_CONFIG) else { return (String::new(), "Manual".into()) };
        let h = Scm(h);
        let mut need = 0u32;
        let mut desc = String::new();
        let _ = QueryServiceConfig2W(h.0, SERVICE_CONFIG_DESCRIPTION, None, &mut need);
        if need > 0 {
            let mut buf = vec![0u8; need as usize];
            if QueryServiceConfig2W(h.0, SERVICE_CONFIG_DESCRIPTION, Some(&mut buf), &mut need).is_ok() {
                let d = &*(buf.as_ptr() as *const SERVICE_DESCRIPTIONW);
                if !d.lpDescription.is_null() {
                    desc = d.lpDescription.to_string().unwrap_or_default();
                }
            }
        }
        let mut st = "Manual".to_string();
        let _ = QueryServiceConfigW(h.0, None, 0, &mut need);
        if need > 0 {
            let mut buf = vec![0u8; need as usize];
            let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if QueryServiceConfigW(h.0, Some(cfg), need, &mut need).is_ok() {
                st = start_type_name((*cfg).dwStartType).to_string();
            }
        }
        (desc, st)
    }
}

pub fn list() -> Result<Vec<Service>, String> {
    let scm = manager(SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT)?;
    let mut out = Vec::new();
    unsafe {
        // Services can be added between the sizing call and the read; page through with the
        // resume handle so a growing list never fails the whole page.
        let mut resume = 0u32;
        loop {
            let mut need = 0u32;
            let mut count = 0u32;
            let mut buf = vec![0u8; 64 * 1024];
            let r = EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, Some(&mut buf), &mut need, &mut count, Some(&mut resume), PCWSTR::null());
            let more = match &r {
                Ok(()) => false,
                Err(e) if e.code() == windows::Win32::Foundation::ERROR_MORE_DATA.to_hresult() => true,
                Err(e) => return Err(e.message().to_string()),
            };
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            for it in items {
                let id = it.lpServiceName.to_string().unwrap_or_default();
                let name = it.lpDisplayName.to_string().unwrap_or_else(|_| id.clone());
                let st = it.ServiceStatusProcess;
                let running = st.dwCurrentState == SERVICE_RUNNING;
                let can_stop = running && (st.dwControlsAccepted & SERVICE_ACCEPT_STOP) != 0;
                let (description, start_type) = details(&scm, &id);
                out.push(Service { id, name, description, running, start_type, can_stop });
            }
            if !more {
                break;
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Starts or stops a service and waits up to ~10 s for it to settle.
pub fn set_running(id: &str, run: bool) -> Result<(), String> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    unsafe {
        let w = wide(id);
        let h = OpenServiceW(scm.0, PCWSTR(w.as_ptr()), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS)
            .map_err(|e| format!("Could not open the service: {}", e.message()))?;
        let h = Scm(h);
        if run {
            StartServiceW(h.0, None).map_err(|e| format!("Could not start the service: {}", e.message()))?;
        } else {
            let mut st = SERVICE_STATUS::default();
            ControlService(h.0, SERVICE_CONTROL_STOP, &mut st).map_err(|e| format!("Could not stop the service: {}", e.message()))?;
        }
        let want = if run { SERVICE_RUNNING } else { SERVICE_STOPPED };
        for _ in 0..60 {
            let mut st = SERVICE_STATUS::default();
            if QueryServiceStatus(h.0, &mut st).is_ok() {
                if st.dwCurrentState == want {
                    return Ok(());
                }
                // A service that was asked to start and fell back to stopped failed to start.
                if run && st.dwCurrentState == SERVICE_STOPPED {
                    return Err("The service stopped right after starting. Check Event Viewer for the reason.".into());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    Err(if run { "The service is still starting after 15 seconds." } else { "The service is still stopping after 15 seconds." }.into())
}
