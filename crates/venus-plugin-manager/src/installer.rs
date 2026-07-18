use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedValue,
};

use crate::publisher::SERVICE_NAME;

const BUS_ITEM_INTERFACE: &str = "com.victronenergy.BusItem";
const SETTINGS_BEGIN: &str = "// BEGIN venus-plugin-manager-settings";
const SETTINGS_END: &str = "// END venus-plugin-manager-settings";
const LEGACY_DASHBOARD_BEGIN: &str = "// BEGIN venus-plugin-manager-dashboards";
const LEGACY_DASHBOARD_END: &str = "// END venus-plugin-manager-dashboards";
const DEVICE_ENTRIES_BEGIN: &str = "// BEGIN venus-plugin-manager-device-entries";
const DEVICE_ENTRIES_END: &str = "// END venus-plugin-manager-device-entries";
const OVERVIEWS_BEGIN: &str = "// BEGIN venus-plugin-manager-overviews";
const OVERVIEWS_END: &str = "// END venus-plugin-manager-overviews";
const RC_BEGIN: &str = "# BEGIN venus-plugin-manager";
const RC_END: &str = "# END venus-plugin-manager";

const NOTIFICATIONS_ENTRY: &str = "\t\t\tMbSubMenu {\n\t\t\t\tid: menuNotifications\n\t\t\t\tdescription: qsTr(\"Notifications\")\n\t\t\t\titem: VBusItem { value: menuNotifications.subpage.summary }\n\t\t\t\tsubpage: PageNotifications { }\n\t\t\t}";
const SETTINGS_ENTRY: &str = "\t\t\tMbSubMenu {\n\t\t\t\tdescription: qsTr(\"Settings\")\n\t\t\t\tsubpage: Component { PageSettings {} }\n\t\t\t}";

const SETTINGS_BLOCK: &str = r#"
		// BEGIN venus-plugin-manager-settings
		MbSubMenu {
			description: qsTr("Plugins")
			subpage: Component { PagePlugins {} }
		}
		// END venus-plugin-manager-settings

"#;

const DEVICE_ENTRIES_BLOCK: &str = r#"
		// BEGIN venus-plugin-manager-device-entries
		PluginDeviceEntriesModel {}
		// END venus-plugin-manager-device-entries

"#;

const OVERVIEWS_BLOCK: &str = r#"
	// BEGIN venus-plugin-manager-overviews
	PluginDashboardController {
		overviewModel: overviewModel
		onAddDashboard: extraOverview(source, true)
	}
	// END venus-plugin-manager-overviews

"#;

const QML_FILES: &[(&str, &str)] = &[
    (
        "PagePlugins.qml",
        include_str!("../../../ui/qml/PagePlugins.qml"),
    ),
    (
        "PagePluginList.qml",
        include_str!("../../../ui/qml/PagePluginList.qml"),
    ),
    (
        "PagePluginDetails.qml",
        include_str!("../../../ui/qml/PagePluginDetails.qml"),
    ),
    (
        "PluginDeviceEntriesModel.qml",
        include_str!("../../../ui/qml/PluginDeviceEntriesModel.qml"),
    ),
    (
        "PluginDashboardController.qml",
        include_str!("../../../ui/qml/PluginDashboardController.qml"),
    ),
];

const OBSOLETE_QML_FILES: &[&str] = &["PagePluginDashboards.qml", "PagePluginDashboardHost.qml"];

#[derive(Debug, Error)]
pub enum InstallerError {
    #[error("unsupported target: {0}")]
    Unsupported(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not find the expected Venus OS v3.55 QML anchor in {0}")]
    MissingAnchor(PathBuf),
    #[error("incomplete Plugin Manager marker block in {0}")]
    BrokenMarker(PathBuf),
    #[error("service path is not owned by Plugin Manager: {0}")]
    OwnershipConflict(PathBuf),
    #[error("command failed: {command}: {message}")]
    Command { command: String, message: String },
    #[error("Plugin Manager D-Bus service did not become healthy")]
    ManagerHealth,
    #[error("Venus GUI did not become healthy")]
    GuiHealth,
}

#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub app_root: PathBuf,
    pub gui_qml_root: PathBuf,
    pub service_root: PathBuf,
    pub manager_service: PathBuf,
    pub rc_local: PathBuf,
    pub version_file: PathBuf,
}

impl InstallConfig {
    pub fn device() -> Self {
        let app_root = PathBuf::from("/data/venus-gx-plugins");
        Self {
            manager_service: app_root.join("service"),
            app_root,
            gui_qml_root: PathBuf::from("/opt/victronenergy/gui/qml"),
            service_root: PathBuf::from("/service"),
            rc_local: PathBuf::from("/data/rc.local"),
            version_file: PathBuf::from("/opt/victronenergy/version"),
        }
    }
}

pub fn install(config: InstallConfig) -> Result<(), InstallerError> {
    validate_target(&config)?;
    install_inner(&config)
}

fn install_inner(config: &InstallConfig) -> Result<(), InstallerError> {
    let binary = config.app_root.join("bin/venus-plugin-manager");
    let run_script = config.manager_service.join("run");
    let service_link = config.service_root.join("venus-plugin-manager");
    let settings_page = config.gui_qml_root.join("PageSettings.qml");
    let main_page = config.gui_qml_root.join("PageMain.qml");
    let overview_main = config.gui_qml_root.join("main.qml");
    let qml_paths: Vec<_> = QML_FILES
        .iter()
        .map(|(name, _)| config.gui_qml_root.join(name))
        .collect();
    let obsolete_qml_paths: Vec<_> = OBSOLETE_QML_FILES
        .iter()
        .map(|name| config.gui_qml_root.join(name))
        .collect();

    let mut file_paths = vec![
        binary.clone(),
        run_script.clone(),
        config.rc_local.clone(),
        settings_page.clone(),
        main_page.clone(),
        overview_main.clone(),
    ];
    file_paths.extend(qml_paths.iter().cloned());
    file_paths.extend(obsolete_qml_paths.iter().cloned());
    let backups = file_paths
        .iter()
        .map(|path| FileBackup::capture(path))
        .collect::<Result<Vec<_>, _>>()?;
    let previous_link = capture_link(&service_link)?;

    let result = (|| {
        for directory in [
            config.app_root.join("bin"),
            config.app_root.join("backup"),
            config.app_root.join("config"),
            config.app_root.join("services"),
            config.manager_service.clone(),
        ] {
            create_dir_all(&directory)?;
        }

        backup_once(
            &settings_page,
            &config
                .app_root
                .join("backup/PageSettings.qml.v3.55.original"),
        )?;
        backup_once(
            &main_page,
            &config.app_root.join("backup/PageMain.qml.v3.55.original"),
        )?;
        backup_once(
            &overview_main,
            &config.app_root.join("backup/main.qml.v3.55.original"),
        )?;

        let current_exe =
            env::current_exe().map_err(|source| io_error("current executable", source))?;
        let executable = fs::read(&current_exe).map_err(|source| io_error(&current_exe, source))?;
        write_atomic(&binary, &executable, 0o755)?;
        let run = format!(
            "#!/bin/sh\nexec 2>&1\nexec {} serve\n",
            shell_quote(&binary.to_string_lossy())
        );
        write_atomic(&run_script, run.as_bytes(), 0o755)?;

        for ((_, contents), path) in QML_FILES.iter().zip(&qml_paths) {
            write_atomic(path, contents.as_bytes(), 0o644)?;
        }
        for path in &obsolete_qml_paths {
            remove_file_if_exists(path)?;
        }

        patch_file(
            &settings_page,
            SETTINGS_BEGIN,
            SETTINGS_END,
            "\n\t\tMbSubMenu {\n\t\t\tdescription: \"Debug\"",
            SETTINGS_BLOCK,
        )?;
        remove_block_if_present(&main_page, LEGACY_DASHBOARD_BEGIN, LEGACY_DASHBOARD_END)?;
        order_device_footer_for_v355(&main_page)?;
        patch_file(
            &main_page,
            DEVICE_ENTRIES_BEGIN,
            DEVICE_ENTRIES_END,
            "\n\t\tVisibleItemModel {",
            DEVICE_ENTRIES_BLOCK,
        )?;
        patch_file(
            &overview_main,
            OVERVIEWS_BEGIN,
            OVERVIEWS_END,
            "\n\tListModel {\n\t\tid: overviewModel",
            OVERVIEWS_BLOCK,
        )?;

        let rc = match fs::read_to_string(&config.rc_local) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => "#!/bin/sh\n".into(),
            Err(source) => return Err(io_error(&config.rc_local, source)),
        };
        let rc_block = format!(
            "\n{RC_BEGIN}\nif [ ! -e {} ] && [ ! -L {} ]; then\n\tln -s {} {}\nfi\n{RC_END}\n",
            shell_quote(&service_link.to_string_lossy()),
            shell_quote(&service_link.to_string_lossy()),
            shell_quote(&config.manager_service.to_string_lossy()),
            shell_quote(&service_link.to_string_lossy())
        );
        let rc = replace_or_append_block(&config.rc_local, &rc, RC_BEGIN, RC_END, &rc_block)?;
        write_atomic(&config.rc_local, rc.as_bytes(), 0o755)?;

        ensure_owned_link(&service_link, &config.manager_service)?;
        if !wait_for_path(&service_link.join("supervise/ok"), Duration::from_secs(10)) {
            return Err(InstallerError::Command {
                command: format!("wait for {}", service_link.display()),
                message: "runit did not discover the service".into(),
            });
        }
        let action = if previous_link.is_some() { "-t" } else { "-u" };
        run_command("svc", &[action, &service_link.to_string_lossy()])?;
        if !wait_for_manager(Duration::from_secs(15)) {
            return Err(InstallerError::ManagerHealth);
        }
        set_manager_gui_ready(false)?;
        if !wait_for_gui_ready_value(false, Duration::from_secs(2)) {
            return Err(InstallerError::ManagerHealth);
        }
        run_command("svc", &["-t", "/service/gui"])?;
        if !wait_for_service("/service/gui", Duration::from_secs(15)) {
            return Err(InstallerError::GuiHealth);
        }
        if !wait_for_gui_ready_value(true, Duration::from_secs(20)) {
            return Err(InstallerError::GuiHealth);
        }
        Ok(())
    })();

    if let Err(error) = result {
        let _ = Command::new("svc")
            .args(["-d", &service_link.to_string_lossy()])
            .status();
        for backup in backups.iter().rev() {
            let _ = backup.restore();
        }
        let _ = restore_link(&service_link, previous_link.as_deref());
        if previous_link.is_some() {
            let _ = Command::new("svc")
                .args(["-u", &service_link.to_string_lossy()])
                .status();
        }
        let _ = Command::new("svc").args(["-t", "/service/gui"]).status();
        return Err(error);
    }
    Ok(())
}

fn validate_target(config: &InstallConfig) -> Result<(), InstallerError> {
    let version = fs::read_to_string(&config.version_file)
        .map_err(|source| io_error(&config.version_file, source))?;
    if !version.lines().any(|line| line.trim() == "v3.55") {
        return Err(InstallerError::Unsupported(
            "only Venus OS v3.55 is supported".into(),
        ));
    }
    let output = Command::new("uname")
        .arg("-m")
        .output()
        .map_err(|source| io_error("uname", source))?;
    let architecture = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || architecture != "armv7l" {
        return Err(InstallerError::Unsupported(format!(
            "only armv7l is supported, found {architecture}"
        )));
    }
    Ok(())
}

fn patch_file(
    path: &Path,
    begin: &str,
    end: &str,
    anchor: &str,
    block: &str,
) -> Result<(), InstallerError> {
    let contents = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    let patched = if contents.contains(begin) || contents.contains(end) {
        replace_block(path, &contents, begin, end, block)?
    } else {
        let index = contents
            .find(anchor)
            .ok_or_else(|| InstallerError::MissingAnchor(path.to_path_buf()))?;
        let mut patched = String::with_capacity(contents.len() + block.len());
        patched.push_str(&contents[..index]);
        patched.push_str(block);
        patched.push_str(&contents[index..]);
        patched
    };
    write_atomic(path, patched.as_bytes(), 0o644)
}

fn remove_block_if_present(path: &Path, begin: &str, end: &str) -> Result<(), InstallerError> {
    let contents = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    if !contents.contains(begin) && !contents.contains(end) {
        return Ok(());
    }
    let patched = replace_block(path, &contents, begin, end, "")?;
    write_atomic(path, patched.as_bytes(), 0o644)
}

fn order_device_footer_for_v355(path: &Path) -> Result<(), InstallerError> {
    let contents = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    let notifications = contents
        .find(NOTIFICATIONS_ENTRY)
        .ok_or_else(|| InstallerError::MissingAnchor(path.to_path_buf()))?;
    let settings = contents
        .find(SETTINGS_ENTRY)
        .ok_or_else(|| InstallerError::MissingAnchor(path.to_path_buf()))?;

    // Preserve the native v3.55 footer order after migrating legacy plugin rows.
    // Plugin Manager inserts its own model before this footer and does not own
    // either of these system menu entries.
    if notifications < settings {
        return Ok(());
    }

    let settings_end = settings + SETTINGS_ENTRY.len();
    let notifications_end = notifications + NOTIFICATIONS_ENTRY.len();
    let patched = format!(
        "{}{}{}{}{}",
        &contents[..settings],
        NOTIFICATIONS_ENTRY,
        &contents[settings_end..notifications],
        SETTINGS_ENTRY,
        &contents[notifications_end..]
    );
    write_atomic(path, patched.as_bytes(), 0o644)
}

fn replace_or_append_block(
    path: &Path,
    contents: &str,
    begin: &str,
    end: &str,
    block: &str,
) -> Result<String, InstallerError> {
    if contents.contains(begin) || contents.contains(end) {
        replace_block(path, contents, begin, end, block)
    } else {
        Ok(format!("{}{block}", contents.trim_end()))
    }
}

fn replace_block(
    path: &Path,
    contents: &str,
    begin: &str,
    end: &str,
    replacement: &str,
) -> Result<String, InstallerError> {
    let start = contents
        .find(begin)
        .ok_or_else(|| InstallerError::BrokenMarker(path.to_path_buf()))?;
    let end_start = contents[start..]
        .find(end)
        .map(|offset| start + offset)
        .ok_or_else(|| InstallerError::BrokenMarker(path.to_path_buf()))?;
    let end_index = end_start + end.len();
    let line_start = contents[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = contents[end_index..]
        .find('\n')
        .map_or(contents.len(), |offset| end_index + offset + 1);
    Ok(format!(
        "{}{}{}",
        &contents[..line_start],
        replacement,
        &contents[line_end..]
    ))
}

fn backup_once(source: &Path, destination: &Path) -> Result<(), InstallerError> {
    if destination.exists() {
        return Ok(());
    }
    let contents = fs::read(source).map_err(|source_error| io_error(source, source_error))?;
    write_atomic(destination, &contents, 0o644)
}

fn remove_file_if_exists(path: &Path) -> Result<(), InstallerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|source| io_error(path, source))
        }
        Ok(_) => Err(InstallerError::OwnershipConflict(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn ensure_owned_link(link: &Path, target: &Path) -> Result<(), InstallerError> {
    create_dir_all(link.parent().unwrap_or_else(|| Path::new("/")))?;
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing = fs::read_link(link).map_err(|source| io_error(link, source))?;
            if existing == target {
                return Ok(());
            }
            return Err(InstallerError::OwnershipConflict(link.to_path_buf()));
        }
        Ok(_) => return Err(InstallerError::OwnershipConflict(link.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error(link, source)),
    }
    symlink(target, link).map_err(|source| io_error(link, source))
}

fn capture_link(path: &Path) -> Result<Option<PathBuf>, InstallerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path)
            .map(Some)
            .map_err(|source| io_error(path, source)),
        Ok(_) => Err(InstallerError::OwnershipConflict(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(path, source)),
    }
}

fn restore_link(path: &Path, target: Option<&Path>) -> Result<(), InstallerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|source| io_error(path, source))?;
        }
        Ok(_) => return Err(InstallerError::OwnershipConflict(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error(path, source)),
    }
    if let Some(target) = target {
        symlink(target, path).map_err(|source| io_error(path, source))?;
    }
    Ok(())
}

fn wait_for_manager(timeout: Duration) -> bool {
    let Ok(connection) = Connection::system() else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let connected = Proxy::new(&connection, SERVICE_NAME, "/Connected", BUS_ITEM_INTERFACE)
            .and_then(|proxy| proxy.call::<_, _, OwnedValue>("GetValue", &()))
            .ok()
            .and_then(|value| i32::try_from(value).ok())
            == Some(1);
        if connected {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn set_manager_gui_ready(ready: bool) -> Result<(), InstallerError> {
    let connection = Connection::system().map_err(|error| InstallerError::Command {
        command: "connect to system D-Bus".into(),
        message: error.to_string(),
    })?;
    let proxy = Proxy::new(&connection, SERVICE_NAME, "/Gui/Ready", BUS_ITEM_INTERFACE).map_err(
        |error| InstallerError::Command {
            command: "open Plugin Manager GUI readiness item".into(),
            message: error.to_string(),
        },
    )?;
    let result = proxy
        .call::<_, _, i32>("SetValue", &(OwnedValue::from(i32::from(ready)),))
        .map_err(|error| InstallerError::Command {
            command: "reset Plugin Manager GUI readiness".into(),
            message: error.to_string(),
        })?;
    if result == 0 {
        Ok(())
    } else {
        Err(InstallerError::Command {
            command: "reset Plugin Manager GUI readiness".into(),
            message: format!("D-Bus result {result}"),
        })
    }
}

fn wait_for_gui_ready_value(expected: bool, timeout: Duration) -> bool {
    let Ok(connection) = Connection::system() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(&connection, SERVICE_NAME, "/Gui/Ready", BUS_ITEM_INTERFACE) else {
        return false;
    };
    let expected = i32::from(expected);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if proxy
            .call::<_, _, OwnedValue>("GetValue", &())
            .ok()
            .and_then(|value| i32::try_from(value).ok())
            == Some(expected)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_for_service(path: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Command::new("svstat")
            .arg(path)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains("up"))
        {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn run_command(program: &str, arguments: &[&str]) -> Result<(), InstallerError> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|source| io_error(program, source))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InstallerError::Command {
            command: format!("{} {}", program, arguments.join(" ")),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn create_dir_all(path: &Path) -> Result<(), InstallerError> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))
}

fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<(), InstallerError> {
    if let Ok(existing) = fs::read(path) {
        let existing_mode = fs::metadata(path)
            .map_err(|source| io_error(path, source))?
            .permissions()
            .mode()
            & 0o777;
        if existing == contents && existing_mode == mode {
            return Ok(());
        }
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|source| io_error(&temp, source))?;
    file.write_all(contents)
        .map_err(|source| io_error(&temp, source))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|source| io_error(&temp, source))?;
    file.sync_all().map_err(|source| io_error(&temp, source))?;
    fs::rename(&temp, path).map_err(|source| io_error(path, source))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn io_error(path: impl Into<PathBuf>, source: io::Error) -> InstallerError {
    InstallerError::Io {
        path: path.into(),
        source,
    }
}

struct FileBackup {
    path: PathBuf,
    contents: Option<Vec<u8>>,
    mode: u32,
}

impl FileBackup {
    fn capture(path: &Path) -> Result<Self, InstallerError> {
        match fs::read(path) {
            Ok(contents) => {
                let mode = fs::metadata(path)
                    .map_err(|source| io_error(path, source))?
                    .permissions()
                    .mode();
                Ok(Self {
                    path: path.to_path_buf(),
                    contents: Some(contents),
                    mode,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                path: path.to_path_buf(),
                contents: None,
                mode: 0,
            }),
            Err(source) => Err(io_error(path, source)),
        }
    }

    fn restore(&self) -> Result<(), InstallerError> {
        if let Some(contents) = &self.contents {
            write_atomic(&self.path, contents, self.mode)
        } else {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(io_error(&self.path, source)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn patches_realistic_qml_once_without_removing_other_changes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("PageSettings.qml");
        fs::write(
            &path,
            "MbPage {\n\tmodel: VisibleItemModel {\n\t\t// custom entry\n\t\tMbSubMenu {\n\t\t\tdescription: \"Debug\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();
        patch_file(
            &path,
            SETTINGS_BEGIN,
            SETTINGS_END,
            "\n\t\tMbSubMenu {\n\t\t\tdescription: \"Debug\"",
            SETTINGS_BLOCK,
        )
        .unwrap();
        patch_file(
            &path,
            SETTINGS_BEGIN,
            SETTINGS_END,
            "\n\t\tMbSubMenu {\n\t\t\tdescription: \"Debug\"",
            SETTINGS_BLOCK,
        )
        .unwrap();
        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.matches(SETTINGS_BEGIN).count(), 1);
        assert_eq!(contents.matches(SETTINGS_END).count(), 1);
        assert!(contents.contains("// custom entry"));
    }

    #[test]
    fn migrates_the_device_list_to_a_single_dynamic_mount() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("PageMain.qml");
        fs::write(
            &path,
            "MbPage {\n\tmodel: VisualModels {\n\t\tVisualDataModel {}\n\t\tVisibleItemModel {\n\t\t\t// BEGIN venus-plugin-manager-dashboards\n\t\t\tMbSubMenu {}\n\t\t\t// END venus-plugin-manager-dashboards\n\t\t\t// unrelated entry\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        remove_block_if_present(&path, LEGACY_DASHBOARD_BEGIN, LEGACY_DASHBOARD_END).unwrap();
        for _ in 0..2 {
            patch_file(
                &path,
                DEVICE_ENTRIES_BEGIN,
                DEVICE_ENTRIES_END,
                "\n\t\tVisibleItemModel {",
                DEVICE_ENTRIES_BLOCK,
            )
            .unwrap();
        }

        let contents = fs::read_to_string(path).unwrap();
        assert!(!contents.contains(LEGACY_DASHBOARD_BEGIN));
        assert_eq!(contents.matches(DEVICE_ENTRIES_BEGIN).count(), 1);
        assert_eq!(contents.matches(DEVICE_ENTRIES_END).count(), 1);
        assert!(contents.contains("PluginDeviceEntriesModel {}"));
        assert!(contents.contains("// unrelated entry"));
    }

    #[test]
    fn keeps_settings_visually_last_in_the_v355_device_list() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("PageMain.qml");
        fs::write(
            &path,
            format!(
                "MbPage {{\n\tmodel: VisibleItemModel {{\n{SETTINGS_ENTRY}\n\n{NOTIFICATIONS_ENTRY}\n\t}}\n}}\n"
            ),
        )
        .unwrap();

        order_device_footer_for_v355(&path).unwrap();
        order_device_footer_for_v355(&path).unwrap();
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.find(NOTIFICATIONS_ENTRY) < contents.find(SETTINGS_ENTRY));
        assert_eq!(contents.matches("id: menuNotifications").count(), 1);
        assert_eq!(
            contents.matches("description: qsTr(\"Settings\")").count(),
            1
        );
    }

    #[test]
    fn identical_atomic_write_keeps_the_existing_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("unchanged");
        fs::write(&path, b"same").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        write_atomic(&path, b"same", 0o644).unwrap();

        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    }

    #[test]
    fn mounts_dashboard_controller_once_without_replacing_native_overviews() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.qml");
        fs::write(
            &path,
            "PageStackWindow {\n\t// native content\n\tListModel {\n\t\tid: overviewModel\n\t\tListElement { pageSource: \"OverviewHub.qml\" }\n\t}\n}\n",
        )
        .unwrap();

        for _ in 0..2 {
            patch_file(
                &path,
                OVERVIEWS_BEGIN,
                OVERVIEWS_END,
                "\n\tListModel {\n\t\tid: overviewModel",
                OVERVIEWS_BLOCK,
            )
            .unwrap();
        }

        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.matches(OVERVIEWS_BEGIN).count(), 1);
        assert_eq!(contents.matches(OVERVIEWS_END).count(), 1);
        assert!(contents.contains("PluginDashboardController"));
        assert!(contents.contains("OverviewHub.qml"));
        assert!(contents.contains("// native content"));
    }

    #[test]
    fn plugin_list_keeps_catalog_descriptions_in_the_details_page() {
        let list = include_str!("../../../ui/qml/PagePluginList.qml");
        let details = include_str!("../../../ui/qml/PagePluginDetails.qml");

        assert!(!list.contains("/Description"));
        assert!(details.contains("/Description"));
    }

    #[test]
    fn plugins_page_shows_the_installed_manager_version_last() {
        let page = include_str!("../../../ui/qml/PagePlugins.qml");
        let update = page.find("Update Plugin Manager").unwrap();
        let version = page.find("/Manager/InstalledVersion").unwrap();

        assert!(version > update);
    }

    #[test]
    fn rejects_an_incomplete_marker_block() {
        let error = replace_block(Path::new("x"), "begin only", "begin", "end", "new").unwrap_err();
        assert!(matches!(error, InstallerError::BrokenMarker(_)));
    }

    #[test]
    fn shell_quotes_paths_without_execution() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
