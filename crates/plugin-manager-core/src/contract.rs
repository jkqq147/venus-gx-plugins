use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CATALOG_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;
const LEGACY_MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_DEVICE_LIST_VALUES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: Runtime,
    pub settings: PluginSettings,
    #[serde(default)]
    pub ui: PluginUi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Runtime {
    NativeService { executable: String },
    QmlOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettings {
    pub enabled_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginUi {
    #[serde(default)]
    pub settings_page: Option<String>,
    #[serde(default)]
    pub dashboard_component: Option<String>,
    #[serde(default)]
    pub device_list: Option<DeviceListUi>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceListUi {
    pub value_paths: Vec<String>,
}

impl PluginUi {
    pub fn is_empty(&self) -> bool {
        self.settings_page.is_none() && self.dashboard_component.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub plugins: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub package: PackageSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSource {
    pub url: String,
    pub sha256: String,
    pub signature: PackageSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub key_id: String,
    pub ed25519: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("unsupported schema version {0}")]
    UnsupportedSchema(u32),
    #[error("invalid plugin id: {0}")]
    InvalidId(String),
    #[error("plugin name must not be empty")]
    EmptyName,
    #[error("invalid semantic version: {0}")]
    InvalidVersion(String),
    #[error("native executable must be below bin/: {0}")]
    InvalidExecutable(String),
    #[error("qml-only plugin must declare at least one QML component")]
    MissingQmlUi,
    #[error("enabled setting must be {expected}, got {actual}")]
    InvalidEnabledPath { expected: String, actual: String },
    #[error("invalid QML asset path: {0}")]
    InvalidQmlPath(String),
    #[error("device_list requires manifest schema {MANIFEST_SCHEMA_VERSION}")]
    DeviceListRequiresCurrentSchema,
    #[error("device_list requires ui.settings_page")]
    DeviceListWithoutSettingsPage,
    #[error("device_list must declare between 1 and {MAX_DEVICE_LIST_VALUES} values, got {0}")]
    InvalidDeviceListValueCount(usize),
    #[error("invalid Device List D-Bus value path: {0}")]
    InvalidDeviceListValuePath(String),
    #[error("duplicate Device List D-Bus value path: {0}")]
    DuplicateDeviceListValuePath(String),
    #[error("duplicate catalog plugin id: {0}")]
    DuplicateCatalogId(String),
    #[error("catalog package URL must use HTTPS: {0}")]
    InsecurePackageUrl(String),
    #[error("invalid SHA-256: {0}")]
    InvalidSha256(String),
    #[error("invalid signing key ID: {0}")]
    InvalidSigningKeyId(String),
    #[error("invalid Ed25519 signature encoding")]
    InvalidSignature,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_manifest_schema(self.schema)?;
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err(ContractError::EmptyName);
        }
        validate_version(&self.version)?;

        if let Runtime::NativeService { executable } = &self.runtime {
            if !is_safe_relative_path(executable, "bin/") {
                return Err(ContractError::InvalidExecutable(executable.clone()));
            }
        } else if self.ui.is_empty() {
            return Err(ContractError::MissingQmlUi);
        }

        let expected_enabled_path = format!("/Settings/Plugins/{}/Enabled", self.id);
        if self.settings.enabled_path != expected_enabled_path {
            return Err(ContractError::InvalidEnabledPath {
                expected: expected_enabled_path,
                actual: self.settings.enabled_path.clone(),
            });
        }

        for path in [&self.ui.settings_page, &self.ui.dashboard_component]
            .into_iter()
            .flatten()
        {
            if !is_safe_relative_path(path, "qml/") || !path.ends_with(".qml") {
                return Err(ContractError::InvalidQmlPath(path.clone()));
            }
        }

        if let Some(device_list) = &self.ui.device_list {
            if self.schema != MANIFEST_SCHEMA_VERSION {
                return Err(ContractError::DeviceListRequiresCurrentSchema);
            }
            if self.ui.settings_page.is_none() {
                return Err(ContractError::DeviceListWithoutSettingsPage);
            }
            if device_list.value_paths.is_empty()
                || device_list.value_paths.len() > MAX_DEVICE_LIST_VALUES
            {
                return Err(ContractError::InvalidDeviceListValueCount(
                    device_list.value_paths.len(),
                ));
            }
            let mut paths = HashSet::new();
            for path in &device_list.value_paths {
                if !is_safe_bus_item_path(path) {
                    return Err(ContractError::InvalidDeviceListValuePath(path.clone()));
                }
                if !paths.insert(path) {
                    return Err(ContractError::DuplicateDeviceListValuePath(path.clone()));
                }
            }
        }

        Ok(())
    }
}

impl Catalog {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_catalog_schema(self.schema)?;
        let mut ids = HashSet::new();
        for plugin in &self.plugins {
            validate_id(&plugin.id)?;
            if !ids.insert(&plugin.id) {
                return Err(ContractError::DuplicateCatalogId(plugin.id.clone()));
            }
            if plugin.name.trim().is_empty() {
                return Err(ContractError::EmptyName);
            }
            validate_version(&plugin.version)?;
            if !plugin.package.url.starts_with("https://") {
                return Err(ContractError::InsecurePackageUrl(
                    plugin.package.url.clone(),
                ));
            }
            if !is_sha256(&plugin.package.sha256) {
                return Err(ContractError::InvalidSha256(plugin.package.sha256.clone()));
            }
            if !valid_key_id(&plugin.package.signature.key_id) {
                return Err(ContractError::InvalidSigningKeyId(
                    plugin.package.signature.key_id.clone(),
                ));
            }
            if !valid_base64_signature(&plugin.package.signature.ed25519) {
                return Err(ContractError::InvalidSignature);
            }
        }
        Ok(())
    }
}

fn validate_catalog_schema(schema: u32) -> Result<(), ContractError> {
    if schema == CATALOG_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ContractError::UnsupportedSchema(schema))
    }
}

fn validate_manifest_schema(schema: u32) -> Result<(), ContractError> {
    if schema == LEGACY_MANIFEST_SCHEMA_VERSION || schema == MANIFEST_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ContractError::UnsupportedSchema(schema))
    }
}

fn validate_id(id: &str) -> Result<(), ContractError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(ContractError::InvalidId(id.to_owned()))
    }
}

fn validate_version(version: &str) -> Result<(), ContractError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut parts = version.split('.');
    let valid = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(ContractError::InvalidVersion(version.to_owned()))
    }
}

fn is_safe_relative_path(path: &str, prefix: &str) -> bool {
    path.starts_with(prefix)
        && !path.contains('\\')
        && !path.bytes().any(|byte| byte.is_ascii_control())
        && !path.contains("//")
        && !path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

fn is_safe_bus_item_path(path: &str) -> bool {
    let Some((service, item_path)) = path.split_once('/') else {
        return false;
    };
    let service_valid = service.len() <= 255
        && service.contains('.')
        && service.split('.').all(|segment| {
            !segment.is_empty()
                && !segment.as_bytes()[0].is_ascii_digit()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        });
    let item_path_valid = !item_path.is_empty()
        && item_path.len() <= 255
        && item_path.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        });
    service_valid && item_path_valid
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_base64_signature(value: &str) -> bool {
    value.len() == 88
        && value.ends_with("==")
        && value[..86]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            version: "0.1.0".into(),
            runtime: Runtime::NativeService {
                executable: "bin/venus-tpms-ble".into(),
            },
            settings: PluginSettings {
                enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
            },
            ui: PluginUi {
                settings_page: Some("qml/PageTpmsSettings.qml".into()),
                dashboard_component: Some("qml/OverviewTpms.qml".into()),
                device_list: None,
            },
        }
    }

    #[test]
    fn accepts_a_native_plugin_manifest() {
        assert_eq!(manifest().validate(), Ok(()));
    }

    #[test]
    fn rejects_a_path_outside_the_plugin_package() {
        let mut manifest = manifest();
        manifest.runtime = Runtime::NativeService {
            executable: "bin/../service".into(),
        };
        assert_eq!(
            manifest.validate(),
            Err(ContractError::InvalidExecutable("bin/../service".into()))
        );
    }

    #[test]
    fn rejects_a_qml_only_plugin_without_ui() {
        let mut manifest = manifest();
        manifest.runtime = Runtime::QmlOnly;
        manifest.ui = PluginUi::default();
        assert_eq!(manifest.validate(), Err(ContractError::MissingQmlUi));
    }

    #[test]
    fn rejects_qml_paths_that_cannot_be_safely_published() {
        for path in ["qml/Overview\\Injected.qml", "qml/Overview\nInjected.qml"] {
            let mut manifest = manifest();
            manifest.ui.dashboard_component = Some(path.into());
            assert_eq!(
                manifest.validate(),
                Err(ContractError::InvalidQmlPath(path.into()))
            );
        }
    }

    #[test]
    fn accepts_four_declarative_device_list_values_in_schema_two() {
        let mut manifest = manifest();
        manifest.schema = MANIFEST_SCHEMA_VERSION;
        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: ["front_left", "front_right", "rear_left", "rear_right"]
                .map(|slot| format!("com.victronenergy.tpms.main/Slots/{slot}/DeviceListValue"))
                .to_vec(),
        });
        assert_eq!(manifest.validate(), Ok(()));
    }

    #[test]
    fn rejects_device_list_values_in_schema_one() {
        let mut manifest = manifest();
        manifest.schema = LEGACY_MANIFEST_SCHEMA_VERSION;
        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: vec![
                "com.victronenergy.tpms.main/Slots/front_left/DeviceListValue".into(),
            ],
        });
        assert_eq!(
            manifest.validate(),
            Err(ContractError::DeviceListRequiresCurrentSchema)
        );
    }

    #[test]
    fn rejects_device_list_values_without_a_settings_page() {
        let mut manifest = manifest();
        manifest.ui.settings_page = None;
        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: vec![
                "com.victronenergy.tpms.main/Slots/front_left/DeviceListValue".into(),
            ],
        });
        assert_eq!(
            manifest.validate(),
            Err(ContractError::DeviceListWithoutSettingsPage)
        );
    }

    #[test]
    fn rejects_unsafe_or_oversized_device_list_values() {
        let mut manifest = manifest();
        manifest.schema = MANIFEST_SCHEMA_VERSION;
        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: vec!["com.victronenergy.tpms.main/Slots/front-left/Value".into()],
        });
        assert!(matches!(
            manifest.validate(),
            Err(ContractError::InvalidDeviceListValuePath(_))
        ));

        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: (0..5)
                .map(|index| format!("com.example.plugin/Values/Value{index}"))
                .collect(),
        });
        assert_eq!(
            manifest.validate(),
            Err(ContractError::InvalidDeviceListValueCount(5))
        );
    }

    #[test]
    fn rejects_duplicate_device_list_values() {
        let mut manifest = manifest();
        let path = "com.victronenergy.tpms.main/Slots/front_left/DeviceListValue";
        manifest.ui.device_list = Some(DeviceListUi {
            value_paths: vec![path.into(), path.into()],
        });
        assert_eq!(
            manifest.validate(),
            Err(ContractError::DuplicateDeviceListValuePath(path.into()))
        );
    }

    #[test]
    fn rejects_unknown_manifest_fields() {
        let mut value = serde_json::to_value(manifest()).unwrap();
        value["install_script"] = serde_json::json!("bin/install.sh");
        assert!(serde_json::from_value::<PluginManifest>(value).is_err());
    }

    #[test]
    fn rejects_duplicate_catalog_entries() {
        let entry = CatalogEntry {
            id: "tpms".into(),
            name: "TPMS".into(),
            version: "0.1.0".into(),
            package: PackageSource {
                url: "https://example.com/tpms.vplugin".into(),
                sha256: "0".repeat(64),
                signature: PackageSignature {
                    key_id: "test-key".into(),
                    ed25519: format!("{}==", "A".repeat(86)),
                },
            },
        };
        let catalog = Catalog {
            schema: CATALOG_SCHEMA_VERSION,
            plugins: vec![entry.clone(), entry],
        };
        assert_eq!(
            catalog.validate(),
            Err(ContractError::DuplicateCatalogId("tpms".into()))
        );
    }
}
