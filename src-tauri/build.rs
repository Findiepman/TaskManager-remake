fn main() {
    // The manifest is "asInvoker" on purpose: the app elevates itself through UAC at start (see
    // src/win/elevate.rs). TM_DEV_AS_INVOKER=1 skips that step for UI testing; never ship it.
    println!("cargo:rerun-if-env-changed=TM_DEV_AS_INVOKER");
    println!("cargo:rerun-if-changed=app.manifest");
    let manifest = include_str!("app.manifest");
    let attrs = tauri_build::Attributes::new().windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(manifest));
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
