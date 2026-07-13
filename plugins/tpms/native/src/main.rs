#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, path::PathBuf};

    if env::var_os("VENUS_TPMS_STATE_PATH").is_none() {
        if let Some(config_root) = env::var_os("VENUS_PLUGIN_CONFIG_DIR") {
            let state_path = PathBuf::from(config_root).join("state.json");
            env::set_var("VENUS_TPMS_STATE_PATH", state_path);
        }
    }
    tpms_core::run_service()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("venus-tpms-ble only runs on Linux");
    std::process::exit(2);
}
