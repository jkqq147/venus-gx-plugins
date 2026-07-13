use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

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
        validate_schema(self.schema)?;
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

        Ok(())
    }
}

impl Catalog {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema)?;
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

fn validate_schema(schema: u32) -> Result<(), ContractError> {
    if schema == SCHEMA_VERSION {
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
        && !path.contains("//")
        && !path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
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
            schema: SCHEMA_VERSION,
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
            schema: SCHEMA_VERSION,
            plugins: vec![entry.clone(), entry],
        };
        assert_eq!(
            catalog.validate(),
            Err(ContractError::DuplicateCatalogId("tpms".into()))
        );
    }
}
