pub mod logic;
#[cfg(windows)]
mod sampler;
#[cfg(windows)]
mod sensors;
#[cfg(windows)]
pub mod win;

#[cfg(windows)]
pub use app::run;

#[cfg(windows)]
mod app {
    use crate::logic;
    use crate::sampler::{Sampler, Snapshot};
    use crate::win::{actions, replace, services, startup};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tauri::{Emitter, Manager};

    #[derive(Default)]
    struct Shared {
        latest: Option<Snapshot>,
        /// (pid, parent, start time) from the last sample.
        procs: Vec<(u32, Option<u32>, u64)>,
        /// Set when system information can't be read at all; shown in the window.
        fatal: Option<String>,
    }

    type State<'a> = tauri::State<'a, Arc<Mutex<Shared>>>;

    #[tauri::command]
    fn snapshot(state: State) -> Result<Option<Snapshot>, String> {
        let s = state.lock();
        match &s.fatal {
            Some(e) => Err(e.clone()),
            None => Ok(s.latest.clone()),
        }
    }

    /// A process the user picked: its pid plus the start time shown on screen. The start time
    /// tells it apart from a newer process that got the same (reused) pid.
    #[derive(serde::Deserialize)]
    struct Target {
        pid: u32,
        start: u64,
    }

    fn kill_all(targets: &[(u32, u64)]) -> Result<(), String> {
        let mut err = None;
        for &(pid, start) in targets {
            if let Err(e) = actions::kill(pid, Some(start)) {
                err.get_or_insert(e);
            }
        }
        err.map_or(Ok(()), Err)
    }

    #[tauri::command(async)]
    fn end_task(targets: Vec<Target>) -> Result<(), String> {
        kill_all(&targets.iter().map(|t| (t.pid, t.start)).collect::<Vec<_>>())
    }

    #[tauri::command(async)]
    fn end_tree(targets: Vec<Target>, state: State) -> Result<(), String> {
        let procs = state.lock().procs.clone();
        let start: HashMap<u32, u64> = procs.iter().map(|&(p, _, s)| (p, s)).collect();
        // Never take down this app's own window processes (WebView2), even when ending the tree
        // of something that started us, like Explorer.
        let own = logic::subtree(std::process::id(), &procs);
        let mut all: Vec<(u32, u64)> = Vec::new();
        for t in targets {
            // Only expand the tree of the process the user actually saw.
            if !start.get(&t.pid).is_some_and(|&s| s.abs_diff(t.start) <= 2) {
                continue;
            }
            for p in logic::subtree(t.pid, &procs) {
                if !own.contains(&p) && !all.iter().any(|x| x.0 == p) {
                    all.push((p, start[&p]));
                }
            }
        }
        // Check the whole tree first: stopping halfway at a critical process would leave a
        // half-ended tree behind.
        if let Some(&(pid, _)) = all.iter().find(|&&(p, _)| actions::is_critical_pid(p, "")) {
            return Err(format!("This tree contains a critical Windows process ({pid}), so nothing was ended."));
        }
        if all.is_empty() {
            return Err("Those processes already ended.".into());
        }
        kill_all(&all)
    }

    #[tauri::command(async)]
    fn open_location(path: String) -> Result<(), String> {
        actions::open_location(&path)
    }

    #[tauri::command(async)]
    fn startup_list() -> Result<Vec<startup::StartupItem>, String> {
        std::panic::catch_unwind(startup::list).map_err(|_| "Could not read the startup apps.".to_string())
    }

    #[tauri::command(async)]
    fn startup_set(id: String, enabled: bool) -> Result<(), String> {
        startup::set_enabled(&id, enabled)
    }

    #[tauri::command(async)]
    fn services_list() -> Result<Vec<services::Service>, String> {
        services::list()
    }

    #[tauri::command(async)]
    fn service_set(id: String, run: bool) -> Result<(), String> {
        services::set_running(&id, run)
    }

    #[tauri::command(async)]
    fn replace_get() -> replace::State {
        replace::state()
    }

    #[tauri::command(async)]
    fn replace_set(on: bool) -> Result<replace::State, String> {
        replace::set_enabled(on)
    }

    #[tauri::command]
    fn is_admin() -> bool {
        crate::win::elevate::is_elevated()
    }

    /// From a copy running without admin rights: start an elevated copy, then close this one.
    #[tauri::command(async)]
    fn restart_as_admin(app: tauri::AppHandle) -> Result<(), String> {
        if crate::win::elevate::run_as_admin(Some(std::process::id())) {
            app.exit(0);
            Ok(())
        } else {
            Err("Administrator rights were not granted.".into())
        }
    }

    /// Runs the bundled official PawnIO installer (its own window, the user clicks through it).
    #[tauri::command(async)]
    fn install_driver(app: tauri::AppHandle) -> Result<(), String> {
        let exe = app
            .path()
            .resolve("PawnIO_setup.exe", tauri::path::BaseDirectory::Resource)
            .map_err(|e| e.to_string())?;
        if !exe.exists() {
            return Err("The driver installer is missing from this build.".into());
        }
        let code = std::process::Command::new(exe).arg("-install").status().map_err(|e| e.to_string())?.code().unwrap_or(-1);
        match code {
            0 | 3010 => Ok(()),
            1223 => Err("The driver install was cancelled.".into()),
            c => Err(format!("The driver installer did not finish (code {c}).")),
        }
    }

    pub fn run() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        tauri::Builder::default()
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                // A second launch (e.g. Ctrl+Shift+Esc while open) brings this window forward.
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }))
            .manage(shared.clone())
            .setup(move |app| {
                replace::repair();
                let handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("sampler".into())
                    .spawn(move || {
                        let mut s = match Sampler::new() {
                            Ok(s) => s,
                            Err(e) => {
                                shared.lock().fatal = Some(e.clone());
                                let _ = handle.emit("fatal", e);
                                return;
                            }
                        };
                        let mut last = Instant::now();
                        let mut panics = 0;
                        loop {
                            std::thread::sleep(Duration::from_millis(1000).saturating_sub(last.elapsed()));
                            let dt = last.elapsed().as_secs_f64().max(0.2);
                            last = Instant::now();
                            // One bad sample (an odd process, a driver hiccup) must not end the
                            // whole app; skip it and try again next second.
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.sample(dt))) {
                                Ok(snap) => {
                                    panics = 0;
                                    {
                                        let mut g = shared.lock();
                                        g.procs = std::mem::take(&mut s.procs);
                                        g.latest = Some(snap.clone());
                                    }
                                    let _ = handle.emit("snapshot", snap);
                                }
                                Err(_) => {
                                    panics += 1;
                                    if panics >= 10 {
                                        let e = "Reading system information keeps failing. Restart the app.".to_string();
                                        shared.lock().fatal = Some(e.clone());
                                        let _ = handle.emit("fatal", e);
                                        return;
                                    }
                                }
                            }
                        }
                    })?;
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                snapshot,
                end_task,
                end_tree,
                open_location,
                startup_list,
                startup_set,
                services_list,
                service_set,
                replace_get,
                replace_set,
                install_driver,
                is_admin,
                restart_as_admin
            ])
            .build(tauri::generate_context!())
            .expect("error while building the app")
            .run(|_app, event| {
                if let tauri::RunEvent::Exit = event {
                    crate::win::netetw::shutdown();
                }
            });
    }
}
