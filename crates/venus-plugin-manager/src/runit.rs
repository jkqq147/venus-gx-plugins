use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use plugin_manager_core::{InstalledPlugin, Runtime, ServiceState};
use thiserror::Error;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("runit command failed: {command} {path}: {message}")]
    Command {
        command: String,
        path: PathBuf,
        message: String,
    },
    #[error("runit path is not owned by Plugin Manager: {0}")]
    OwnershipConflict(PathBuf),
}

pub trait PluginRuntime {
    fn sync_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError>;
    fn remove_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError>;
    fn purge_config(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError>;
    fn observe(&self, plugin: &InstalledPlugin) -> Result<ServiceState, RuntimeError>;
    fn start(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError>;
    fn stop(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError>;
}

pub trait RunitController: Send + Sync {
    fn control(&self, action: &str, service: &Path) -> Result<(), RuntimeError>;
    fn status(&self, service: &Path) -> Result<String, RuntimeError>;
}

#[derive(Debug, Clone, Default)]
pub struct SystemRunitController;

impl RunitController for SystemRunitController {
    fn control(&self, action: &str, service: &Path) -> Result<(), RuntimeError> {
        let output = Command::new("svc")
            .arg(action)
            .arg(service)
            .output()
            .map_err(|source| io_error(PathBuf::from("svc"), source))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::Command {
                command: format!("svc {action}"),
                path: service.to_path_buf(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    fn status(&self, service: &Path) -> Result<String, RuntimeError> {
        let output = Command::new("svstat")
            .arg(service)
            .output()
            .map_err(|source| io_error(PathBuf::from("svstat"), source))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RuntimeError::Command {
                command: "svstat".into(),
                path: service.to_path_buf(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

pub struct RunitRuntime<C = SystemRunitController> {
    state_root: PathBuf,
    config_root: PathBuf,
    definitions_root: PathBuf,
    service_root: PathBuf,
    controller: C,
}

impl RunitRuntime<SystemRunitController> {
    pub fn new(
        state_root: impl Into<PathBuf>,
        config_root: impl Into<PathBuf>,
        definitions_root: impl Into<PathBuf>,
        service_root: impl Into<PathBuf>,
    ) -> Self {
        Self::with_controller(
            state_root,
            config_root,
            definitions_root,
            service_root,
            SystemRunitController,
        )
    }
}

impl<C: RunitController> RunitRuntime<C> {
    pub fn with_controller(
        state_root: impl Into<PathBuf>,
        config_root: impl Into<PathBuf>,
        definitions_root: impl Into<PathBuf>,
        service_root: impl Into<PathBuf>,
        controller: C,
    ) -> Self {
        Self {
            state_root: state_root.into(),
            config_root: config_root.into(),
            definitions_root: definitions_root.into(),
            service_root: service_root.into(),
            controller,
        }
    }

    fn definition(&self, id: &str) -> PathBuf {
        self.definitions_root.join(id)
    }

    fn service_link(&self, id: &str) -> PathBuf {
        self.service_root.join(format!("venus-plugin-{id}"))
    }

    fn ensure_config_directory(&self, id: &str) -> Result<PathBuf, RuntimeError> {
        fs::create_dir_all(&self.config_root)
            .map_err(|source| io_error(self.config_root.clone(), source))?;
        require_owned_directory(&self.config_root)?;

        let config = self.config_root.join(id);
        match fs::create_dir(&config) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                require_owned_directory(&config)?;
            }
            Err(source) => return Err(io_error(config, source)),
        }
        let mode = fs::symlink_metadata(&config)
            .map_err(|source| io_error(config.clone(), source))?
            .permissions()
            .mode()
            & 0o777;
        if mode != 0o700 {
            fs::set_permissions(&config, fs::Permissions::from_mode(0o700))
                .map_err(|source| io_error(config.clone(), source))?;
        }
        Ok(config)
    }

    fn ensure_owned_link(&self, id: &str) -> Result<(), RuntimeError> {
        let definition = self.definition(id);
        let link = self.service_link(id);
        fs::create_dir_all(&self.service_root)
            .map_err(|source| io_error(self.service_root.clone(), source))?;
        match fs::symlink_metadata(&link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target =
                    fs::read_link(&link).map_err(|source| io_error(link.clone(), source))?;
                if target == definition {
                    return Ok(());
                }
                fs::remove_file(&link).map_err(|source| io_error(link.clone(), source))?;
            }
            Ok(_) => return Err(RuntimeError::OwnershipConflict(link)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(link, source)),
        }
        std::os::unix::fs::symlink(&definition, &link).map_err(|source| io_error(link, source))
    }

    fn remove_owned_link(&self, id: &str) -> Result<(), RuntimeError> {
        let definition = self.definition(id);
        let link = self.service_link(id);
        match fs::symlink_metadata(&link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target =
                    fs::read_link(&link).map_err(|source| io_error(link.clone(), source))?;
                if target != definition {
                    return Err(RuntimeError::OwnershipConflict(link));
                }
                fs::remove_file(&link).map_err(|source| io_error(link, source))
            }
            Ok(_) => Err(RuntimeError::OwnershipConflict(link)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(link, source)),
        }
    }
}

impl<C: RunitController> PluginRuntime for RunitRuntime<C> {
    fn sync_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
        let Runtime::NativeService {
            executable,
            arguments,
            ..
        } = &plugin.manifest.runtime
        else {
            return self.remove_definition(plugin);
        };
        let definition = self.definition(&plugin.manifest.id);
        let is_new = !definition.exists();
        fs::create_dir_all(&definition).map_err(|source| io_error(definition.clone(), source))?;
        let config = self.ensure_config_directory(&plugin.manifest.id)?;
        let binary = self.state_root.join(&plugin.install_path).join(executable);
        let arguments = arguments
            .iter()
            .map(|argument| shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" ");
        let arguments = if arguments.is_empty() {
            String::new()
        } else {
            format!(" {arguments}")
        };
        let script = format!(
            "#!/bin/sh\nset -eu\numask 077\nexport VENUS_PLUGIN_ID={}\nexport VENUS_PLUGIN_CONFIG_DIR={}\ncd {}\nexec {}{}\n",
            shell_quote(&plugin.manifest.id),
            shell_quote(&config.to_string_lossy()),
            shell_quote(&config.to_string_lossy()),
            shell_quote(&binary.to_string_lossy()),
            arguments,
        );
        write_atomic_if_changed(&definition.join("run"), script.as_bytes(), 0o755)?;
        if is_new {
            write_atomic(&definition.join("down"), b"", 0o644)?;
        }
        self.ensure_owned_link(&plugin.manifest.id)
    }

    fn remove_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
        if matches!(plugin.manifest.runtime, Runtime::QmlOnly) {
            return Ok(());
        }
        let link = self.service_link(&plugin.manifest.id);
        if fs::symlink_metadata(&link).is_ok() {
            let _ = self.controller.control("-dx", &link);
        }
        self.remove_owned_link(&plugin.manifest.id)?;
        let definition = self.definition(&plugin.manifest.id);
        match fs::remove_dir_all(&definition) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(definition, source)),
        }
    }

    fn purge_config(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
        match fs::symlink_metadata(&self.config_root) {
            Ok(_) => require_owned_directory(&self.config_root)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error(self.config_root.clone(), source)),
        }

        let config = self.config_root.join(&plugin.manifest.id);
        match fs::symlink_metadata(&config) {
            Ok(_) => require_owned_directory(&config)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error(config, source)),
        }
        fs::remove_dir_all(&config).map_err(|source| io_error(config, source))
    }

    fn observe(&self, plugin: &InstalledPlugin) -> Result<ServiceState, RuntimeError> {
        if matches!(plugin.manifest.runtime, Runtime::QmlOnly) {
            return Ok(ServiceState::NotApplicable);
        }
        let link = self.service_link(&plugin.manifest.id);
        if fs::symlink_metadata(&link).is_err() {
            return Ok(ServiceState::Failed);
        }
        if let Ok(status) = self.controller.status(&link) {
            if status.starts_with("up ") || status.contains(": up ") {
                return Ok(ServiceState::Running);
            }
            if status.starts_with("down ") || status.contains(": down ") {
                return Ok(ServiceState::Stopped);
            }
        }
        if self.definition(&plugin.manifest.id).join("down").is_file() {
            Ok(ServiceState::Stopped)
        } else {
            Ok(ServiceState::Failed)
        }
    }

    fn start(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
        if matches!(plugin.manifest.runtime, Runtime::QmlOnly) {
            return Ok(());
        }
        let down = self.definition(&plugin.manifest.id).join("down");
        match fs::remove_file(&down) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(down, source)),
        }
        self.controller
            .control("-u", &self.service_link(&plugin.manifest.id))
    }

    fn stop(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
        if matches!(plugin.manifest.runtime, Runtime::QmlOnly) {
            return Ok(());
        }
        let definition = self.definition(&plugin.manifest.id);
        fs::create_dir_all(&definition).map_err(|source| io_error(definition.clone(), source))?;
        write_atomic(&definition.join("down"), b"", 0o644)?;
        let link = self.service_link(&plugin.manifest.id);
        if fs::symlink_metadata(&link).is_err() {
            return Ok(());
        }
        self.controller.control("-d", &link)
    }
}

fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<(), RuntimeError> {
    let parent = path
        .parent()
        .ok_or_else(|| RuntimeError::OwnershipConflict(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| io_error(parent.to_path_buf(), source))?;
    let temp = parent.join(format!(".tmp-{}", next_suffix()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|source| io_error(temp.clone(), source))?;
    let result = file
        .write_all(contents)
        .map_err(|source| io_error(temp.clone(), source))
        .and_then(|_| {
            file.sync_all()
                .map_err(|source| io_error(temp.clone(), source))
        })
        .and_then(|_| {
            fs::set_permissions(&temp, fs::Permissions::from_mode(mode))
                .map_err(|source| io_error(temp.clone(), source))
        })
        .and_then(|_| {
            fs::rename(&temp, path).map_err(|source| io_error(path.to_path_buf(), source))
        });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_atomic_if_changed(path: &Path, contents: &[u8], mode: u32) -> Result<(), RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let current = fs::read(path).map_err(|source| io_error(path.to_path_buf(), source))?;
            let current_mode = metadata.permissions().mode() & 0o777;
            if current == contents && current_mode == mode {
                return Ok(());
            }
        }
        Ok(_) => return Err(RuntimeError::OwnershipConflict(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error(path.to_path_buf(), source)),
    }
    write_atomic(path, contents, mode)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn next_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn io_error(path: PathBuf, source: io::Error) -> RuntimeError {
    RuntimeError::Io { path, source }
}

fn require_owned_directory(path: &Path) -> Result<(), RuntimeError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| io_error(path.to_path_buf(), source))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(RuntimeError::OwnershipConflict(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::MetadataExt, sync::Mutex, thread, time::Duration};

    use plugin_manager_core::{PluginManifest, PluginSettings, PluginUi, MANIFEST_SCHEMA_VERSION};
    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeController {
        calls: Mutex<Vec<String>>,
        status: Mutex<String>,
    }

    impl RunitController for FakeController {
        fn control(&self, action: &str, service: &Path) -> Result<(), RuntimeError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{action} {}", service.display()));
            Ok(())
        }

        fn status(&self, _service: &Path) -> Result<String, RuntimeError> {
            Ok(self.status.lock().unwrap().clone())
        }
    }

    fn installed() -> InstalledPlugin {
        InstalledPlugin {
            manifest: PluginManifest {
                schema: MANIFEST_SCHEMA_VERSION,
                id: "tpms".into(),
                name: "TPMS".into(),
                description: "Bluetooth tire pressure monitoring".into(),
                version: "0.1.0".into(),
                runtime: Runtime::NativeService {
                    executable: "bin/tpms".into(),
                    arguments: Vec::new(),
                    companion_executables: Vec::new(),
                },
                settings: PluginSettings {
                    enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
                },
                ui: PluginUi::default(),
            },
            package_sha256: "0".repeat(64),
            install_path: format!("plugins/tpms/{}", "0".repeat(64)),
        }
    }

    #[test]
    fn creates_owned_service_disabled_by_default() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let plugin = installed();

        runtime.sync_definition(&plugin).unwrap();

        let definition = temp.path().join("definitions/tpms");
        assert!(definition.join("down").is_file());
        let script = fs::read_to_string(definition.join("run")).unwrap();
        assert!(script.contains("state/plugins/tpms"));
        assert!(script.contains("\numask 077\n"));
        assert!(script.contains("VENUS_PLUGIN_CONFIG_DIR"));
        assert!(script.contains("\ncd '"));
        assert_eq!(
            fs::metadata(temp.path().join("config/tpms"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::read_link(temp.path().join("service/venus-plugin-tpms")).unwrap(),
            definition
        );
    }

    #[test]
    fn disabled_service_is_stopped_before_supervisor_reports_status() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let plugin = installed();

        runtime.sync_definition(&plugin).unwrap();

        assert_eq!(runtime.observe(&plugin).unwrap(), ServiceState::Stopped);
    }

    #[test]
    fn passes_declared_arguments_without_shell_expansion() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let mut plugin = installed();
        plugin.manifest.runtime = Runtime::NativeService {
            executable: "bin/rathole".into(),
            arguments: vec!["--client".into(), "client.toml".into(), "$HOME".into()],
            companion_executables: Vec::new(),
        };

        runtime.sync_definition(&plugin).unwrap();

        let script = fs::read_to_string(temp.path().join("definitions/tpms/run")).unwrap();
        assert!(script.contains(" '--client' 'client.toml' '$HOME'\n"));
    }

    #[test]
    fn repeated_sync_does_not_touch_unchanged_files_or_permissions() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let plugin = installed();
        runtime.sync_definition(&plugin).unwrap();

        let run = temp.path().join("definitions/tpms/run");
        let config = temp.path().join("config/tpms");
        let run_before = fs::metadata(&run).unwrap();
        let config_before = fs::metadata(&config).unwrap();
        thread::sleep(Duration::from_millis(20));

        runtime.sync_definition(&plugin).unwrap();

        let run_after = fs::metadata(&run).unwrap();
        let config_after = fs::metadata(&config).unwrap();
        assert_eq!(run_before.ino(), run_after.ino());
        assert_eq!(
            run_before.modified().unwrap(),
            run_after.modified().unwrap()
        );
        assert_eq!(
            (config_before.ctime(), config_before.ctime_nsec()),
            (config_after.ctime(), config_after.ctime_nsec())
        );
    }

    #[test]
    fn start_and_stop_keep_runit_default_in_sync() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let plugin = installed();
        runtime.sync_definition(&plugin).unwrap();

        runtime.start(&plugin).unwrap();
        assert!(!temp.path().join("definitions/tpms/down").exists());
        runtime.stop(&plugin).unwrap();
        assert!(temp.path().join("definitions/tpms/down").is_file());
        assert_eq!(runtime.controller.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn parses_runit_status_and_removes_owned_definition() {
        let temp = TempDir::new().unwrap();
        let controller = FakeController::default();
        *controller.status.lock().unwrap() =
            "/service/venus-plugin-tpms: up (pid 7) 2 seconds".into();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            controller,
        );
        let plugin = installed();
        runtime.sync_definition(&plugin).unwrap();

        assert_eq!(runtime.observe(&plugin).unwrap(), ServiceState::Running);
        runtime.remove_definition(&plugin).unwrap();
        assert!(!temp.path().join("definitions/tpms").exists());
        assert!(!temp.path().join("service/venus-plugin-tpms").exists());
    }

    #[test]
    fn uninstall_keeps_config_until_explicit_purge() {
        let temp = TempDir::new().unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            temp.path().join("config"),
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );
        let plugin = installed();
        runtime.sync_definition(&plugin).unwrap();
        let state = temp.path().join("config/tpms/state.json");
        fs::write(&state, b"{}").unwrap();

        runtime.remove_definition(&plugin).unwrap();
        assert_eq!(fs::read(&state).unwrap(), b"{}");

        runtime.purge_config(&plugin).unwrap();
        assert!(!temp.path().join("config/tpms").exists());
    }

    #[test]
    fn purge_rejects_a_symlinked_config_directory() {
        let temp = TempDir::new().unwrap();
        let external = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        fs::create_dir(&config_root).unwrap();
        std::os::unix::fs::symlink(external.path(), config_root.join("tpms")).unwrap();
        let runtime = RunitRuntime::with_controller(
            temp.path().join("state"),
            &config_root,
            temp.path().join("definitions"),
            temp.path().join("service"),
            FakeController::default(),
        );

        let error = runtime.purge_config(&installed()).unwrap_err();
        assert!(matches!(error, RuntimeError::OwnershipConflict(_)));
        assert!(external.path().exists());
    }
}
