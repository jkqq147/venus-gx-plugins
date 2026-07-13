use plugin_manager_core::PluginManifest;
use thiserror::Error;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedValue,
};

const SETTINGS_SERVICE: &str = "com.victronenergy.settings";
const SETTINGS_INTERFACE: &str = "com.victronenergy.Settings";
const BUS_ITEM_INTERFACE: &str = "com.victronenergy.BusItem";

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("D-Bus settings operation failed: {0}")]
    Dbus(#[from] zbus::Error),
    #[error("Venus Settings rejected {path} with status {status}")]
    Rejected { path: String, status: i32 },
    #[error("Venus Settings returned an invalid value for {path}")]
    InvalidValue { path: String },
}

pub trait SettingsStore {
    fn ensure_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError>;
    fn read_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError>;
    fn write_enabled(&self, manifest: &PluginManifest, enabled: bool) -> Result<(), SettingsError>;
    fn remove_enabled(&self, manifest: &PluginManifest) -> Result<(), SettingsError>;
}

#[derive(Clone)]
pub struct VenusSettings {
    connection: Connection,
}

impl VenusSettings {
    pub fn system() -> Result<Self, SettingsError> {
        Ok(Self {
            connection: Connection::system()?,
        })
    }

    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    fn settings_proxy(&self) -> Result<Proxy<'_>, SettingsError> {
        Ok(Proxy::new(
            &self.connection,
            SETTINGS_SERVICE,
            "/Settings",
            SETTINGS_INTERFACE,
        )?)
    }

    fn item_proxy(&self, path: &str) -> Result<Proxy<'_>, SettingsError> {
        Ok(Proxy::new(
            &self.connection,
            SETTINGS_SERVICE,
            path.to_owned(),
            BUS_ITEM_INTERFACE,
        )?)
    }
}

impl SettingsStore for VenusSettings {
    fn ensure_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError> {
        if let Ok(enabled) = self.read_enabled(manifest) {
            return Ok(enabled);
        }
        let relative = relative_setting_path(manifest);
        let status: i32 = self.settings_proxy()?.call(
            "AddSetting",
            &(
                "Plugins",
                format!("{}/Enabled", manifest.id),
                OwnedValue::from(0_i32),
                "i",
                OwnedValue::from(0_i32),
                OwnedValue::from(1_i32),
            ),
        )?;
        if status != 0 {
            return Err(SettingsError::Rejected {
                path: relative,
                status,
            });
        }
        self.read_enabled(manifest)
    }

    fn read_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError> {
        let path = &manifest.settings.enabled_path;
        let value: OwnedValue = self.item_proxy(path)?.call("GetValue", &())?;
        i32::try_from(value)
            .map(|value| value != 0)
            .map_err(|_| SettingsError::InvalidValue { path: path.clone() })
    }

    fn write_enabled(&self, manifest: &PluginManifest, enabled: bool) -> Result<(), SettingsError> {
        let path = &manifest.settings.enabled_path;
        let status: i32 = self.item_proxy(path)?.call(
            "SetValue",
            &OwnedValue::from(if enabled { 1_i32 } else { 0_i32 }),
        )?;
        if status == 0 {
            Ok(())
        } else {
            Err(SettingsError::Rejected {
                path: path.clone(),
                status,
            })
        }
    }

    fn remove_enabled(&self, manifest: &PluginManifest) -> Result<(), SettingsError> {
        let relative = relative_setting_path(manifest);
        let statuses: Vec<i32> = self
            .settings_proxy()?
            .call("RemoveSettings", &vec![relative.clone()])?;
        match statuses.as_slice() {
            [0] => Ok(()),
            [status] => Err(SettingsError::Rejected {
                path: relative,
                status: *status,
            }),
            _ => Err(SettingsError::InvalidValue { path: relative }),
        }
    }
}

fn relative_setting_path(manifest: &PluginManifest) -> String {
    manifest
        .settings
        .enabled_path
        .strip_prefix("/Settings/")
        .unwrap_or(&manifest.settings.enabled_path)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use plugin_manager_core::{PluginSettings, PluginUi, Runtime, MANIFEST_SCHEMA_VERSION};

    use super::*;

    #[test]
    fn derives_relative_path_from_valid_manifest() {
        let manifest = PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            description: "Bluetooth tire pressure monitoring".into(),
            version: "0.1.0".into(),
            runtime: Runtime::QmlOnly,
            settings: PluginSettings {
                enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
            },
            ui: PluginUi {
                settings_page: Some("qml/PageTpms.qml".into()),
                dashboard_component: None,
                device_list: None,
            },
        };
        assert_eq!(relative_setting_path(&manifest), "Plugins/tpms/Enabled");
    }
}
