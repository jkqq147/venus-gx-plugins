use std::{fs, path::Path};

use crate::{
    package::{prepare_package, PackageExpectation},
    registry::{
        create_dir_all, install_path, prune_empty_plugin_dir, remove_dir_all, require_directory,
        InstalledPlugin, LocalRegistry,
    },
    CoreError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Updated,
    Unchanged,
}

impl LocalRegistry {
    pub fn install_vplugin(
        &self,
        package_path: &Path,
        expectation: &PackageExpectation,
    ) -> Result<InstallOutcome, CoreError> {
        self.with_exclusive_lock(|| {
            let mut registry = self.read_registry_for_update_locked()?;
            let transaction_dir = self.create_transaction_dir()?;
            let result = (|| {
                let prepared = prepare_package(package_path, &transaction_dir, expectation)?;
                let previous = registry.plugins.get(&prepared.manifest.id).cloned();
                if previous.as_ref().is_some_and(|plugin| {
                    plugin.package_sha256 == prepared.sha256 && plugin.manifest == prepared.manifest
                }) {
                    return Ok(InstallOutcome::Unchanged);
                }

                let relative_install_path = install_path(&prepared.manifest.id, &prepared.sha256);
                let final_path = self.root.join(&relative_install_path);
                if previous
                    .as_ref()
                    .is_some_and(|plugin| plugin.install_path == relative_install_path)
                {
                    return Err(CoreError::InvalidPackage(
                        "package digest matches the installed payload but its manifest differs"
                            .into(),
                    ));
                }
                if final_path.exists() {
                    remove_dir_all(&final_path)?;
                }
                let parent = final_path.parent().ok_or_else(|| {
                    CoreError::InvalidRegistry("install path has no parent".into())
                })?;
                create_dir_all(parent)?;
                require_directory(parent)?;
                fs::rename(&prepared.payload, &final_path)
                    .map_err(|source| crate::error::io_error(final_path.clone(), source))?;

                registry.plugins.insert(
                    prepared.manifest.id.clone(),
                    InstalledPlugin {
                        manifest: prepared.manifest.clone(),
                        package_sha256: prepared.sha256,
                        install_path: relative_install_path,
                    },
                );

                if let Err(error) = self.write_registry_atomic(&registry) {
                    let _ = fs::remove_dir_all(&final_path);
                    return Err(error);
                }

                if let Some(previous) = &previous {
                    if previous.install_path != registry.plugins[&prepared.manifest.id].install_path
                    {
                        let _ = fs::remove_dir_all(self.root.join(&previous.install_path));
                    }
                }
                prune_empty_plugin_dir(&self.root, &prepared.manifest.id);

                Ok(if previous.is_some() {
                    InstallOutcome::Updated
                } else {
                    InstallOutcome::Installed
                })
            })();
            let _ = fs::remove_dir_all(&transaction_dir);
            result
        })
    }

    pub fn uninstall(&self, id: &str) -> Result<InstalledPlugin, CoreError> {
        self.with_exclusive_lock(|| {
            let mut registry = self.read_registry_for_update_locked()?;
            let removed = registry
                .plugins
                .remove(id)
                .ok_or_else(|| CoreError::NotInstalled(id.to_owned()))?;
            self.write_registry_atomic(&registry)?;

            let _ = fs::remove_dir_all(self.root.join(&removed.install_path));
            prune_empty_plugin_dir(&self.root, id);
            Ok(removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        path::{Path, PathBuf},
    };

    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use crate::{
        PluginManifest, PluginRegistry, PluginSettings, PluginUi, Runtime, MANIFEST_SCHEMA_VERSION,
    };

    use super::*;

    fn native_manifest(version: &str) -> PluginManifest {
        PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            description: "Bluetooth tire pressure monitoring".into(),
            version: version.into(),
            runtime: Runtime::NativeService {
                executable: "bin/venus-tpms-ble".into(),
                arguments: Vec::new(),
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

    fn append_file(builder: &mut tar::Builder<GzEncoder<File>>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o777);
        header.set_cksum();
        builder.append_data(&mut header, path, contents).unwrap();
    }

    fn write_package(
        directory: &Path,
        manifest: &PluginManifest,
        include_runtime: bool,
    ) -> (PathBuf, String) {
        let package = directory.join(format!("{}.vplugin", manifest.version));
        let file = File::create(&package).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_file(
            &mut builder,
            "manifest.json",
            &serde_json::to_vec_pretty(manifest).unwrap(),
        );
        if include_runtime {
            append_file(&mut builder, "bin/venus-tpms-ble", b"test binary");
        }
        append_file(&mut builder, "qml/PageTpmsSettings.qml", b"Item {}");
        append_file(&mut builder, "qml/OverviewTpms.qml", b"Item {}");
        builder.into_inner().unwrap().finish().unwrap();
        let digest = format!("{:x}", Sha256::digest(fs::read(&package).unwrap()));
        (package, digest)
    }

    fn write_package_with_symlink(
        directory: &Path,
        manifest: &PluginManifest,
    ) -> (PathBuf, String) {
        let package = directory.join("symlink.vplugin");
        let file = File::create(&package).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_file(
            &mut builder,
            "manifest.json",
            &serde_json::to_vec_pretty(manifest).unwrap(),
        );
        append_file(&mut builder, "bin/venus-tpms-ble", b"test binary");
        append_file(&mut builder, "qml/PageTpmsSettings.qml", b"Item {}");
        append_file(&mut builder, "qml/OverviewTpms.qml", b"Item {}");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "qml/escape", "/etc/passwd")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let digest = format!("{:x}", Sha256::digest(fs::read(&package).unwrap()));
        (package, digest)
    }

    fn expectation(manifest: &PluginManifest, sha256: &str) -> PackageExpectation {
        PackageExpectation {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            sha256: sha256.into(),
        }
    }

    #[test]
    fn initializes_an_empty_registry() {
        let temp = TempDir::new().unwrap();
        let manager = LocalRegistry::new(temp.path().join("state"));

        assert_eq!(manager.initialize().unwrap(), PluginRegistry::default());
        assert!(manager.root().join("registry.json").is_file());
        assert_eq!(manager.load().unwrap(), PluginRegistry::default());
    }

    #[test]
    fn installs_a_verified_package_without_enabled_state() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));

        assert_eq!(
            manager
                .install_vplugin(&package, &expectation(&manifest, &sha256))
                .unwrap(),
            InstallOutcome::Installed
        );
        let registry = manager.load().unwrap();
        let installed = &registry.plugins["tpms"];
        assert_eq!(installed.manifest, manifest);
        let json = fs::read_to_string(manager.root().join("registry.json")).unwrap();
        assert!(!json.contains("\"enabled\""));

        let mut legacy_value = serde_json::to_value(installed).unwrap();
        legacy_value["enabled"] = serde_json::json!(true);
        assert!(serde_json::from_value::<InstalledPlugin>(legacy_value).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = manager
                .root()
                .join(&installed.install_path)
                .join("bin/venus-tpms-ble");
            assert_eq!(
                fs::metadata(executable).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn upgrade_replaces_payload_without_owning_enabled_state() {
        let temp = TempDir::new().unwrap();
        let first_manifest = native_manifest("0.1.0");
        let (first_package, first_sha) = write_package(temp.path(), &first_manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager
            .install_vplugin(&first_package, &expectation(&first_manifest, &first_sha))
            .unwrap();
        let old_path = manager.load().unwrap().plugins["tpms"].install_path.clone();

        let second_manifest = native_manifest("0.2.0");
        let (second_package, second_sha) = write_package(temp.path(), &second_manifest, true);
        assert_eq!(
            manager
                .install_vplugin(&second_package, &expectation(&second_manifest, &second_sha))
                .unwrap(),
            InstallOutcome::Updated
        );

        let installed = manager.load().unwrap().plugins.remove("tpms").unwrap();
        assert_eq!(installed.manifest.version, "0.2.0");
        assert!(!manager.root().join(old_path).exists());
    }

    #[test]
    fn checksum_failure_does_not_change_registry() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, _) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager.initialize().unwrap();

        let error = manager
            .install_vplugin(&package, &expectation(&manifest, &"0".repeat(64)))
            .unwrap_err();
        assert!(matches!(error, CoreError::ChecksumMismatch { .. }));
        assert!(manager.load().unwrap().plugins.is_empty());
    }

    #[test]
    fn identity_mismatch_does_not_install_package() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        let mut wrong_expectation = expectation(&manifest, &sha256);
        wrong_expectation.id = "rathole".into();

        let error = manager
            .install_vplugin(&package, &wrong_expectation)
            .unwrap_err();
        assert!(matches!(
            error,
            CoreError::IdentityMismatch { field: "id", .. }
        ));
        assert!(manager.load().unwrap().plugins.is_empty());
    }

    #[test]
    fn package_symlinks_are_rejected_before_installation() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package_with_symlink(temp.path(), &manifest);
        let manager = LocalRegistry::new(temp.path().join("state"));

        let error = manager
            .install_vplugin(&package, &expectation(&manifest, &sha256))
            .unwrap_err();
        assert!(matches!(error, CoreError::InvalidPackage(_)));
        assert!(manager.load().unwrap().plugins.is_empty());
    }

    #[test]
    fn reinstalling_identical_package_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        let expected = expectation(&manifest, &sha256);
        manager.install_vplugin(&package, &expected).unwrap();

        assert_eq!(
            manager.install_vplugin(&package, &expected).unwrap(),
            InstallOutcome::Unchanged
        );
    }

    #[test]
    fn next_exclusive_operation_cleans_abandoned_files() {
        let temp = TempDir::new().unwrap();
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager.initialize().unwrap();
        let abandoned_staging = manager.root().join("staging/transaction-abandoned");
        fs::create_dir_all(&abandoned_staging).unwrap();
        fs::write(abandoned_staging.join("package.vplugin"), b"partial").unwrap();
        let abandoned_payload = manager.root().join("plugins/tpms/orphan");
        fs::create_dir_all(&abandoned_payload).unwrap();
        fs::write(abandoned_payload.join("partial"), b"partial").unwrap();
        let abandoned_registry = manager.root().join(".registry.json.tmp-abandoned");
        fs::write(&abandoned_registry, b"partial").unwrap();

        manager.initialize().unwrap();

        assert!(!abandoned_staging.exists());
        assert!(!abandoned_payload.exists());
        assert!(!abandoned_registry.exists());
    }

    #[test]
    fn failed_upgrade_keeps_the_previous_installation() {
        let temp = TempDir::new().unwrap();
        let first_manifest = native_manifest("0.1.0");
        let (first_package, first_sha) = write_package(temp.path(), &first_manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager
            .install_vplugin(&first_package, &expectation(&first_manifest, &first_sha))
            .unwrap();
        let before = manager.load().unwrap();

        let broken_manifest = native_manifest("0.2.0");
        let (broken_package, broken_sha) = write_package(temp.path(), &broken_manifest, false);
        assert!(manager
            .install_vplugin(&broken_package, &expectation(&broken_manifest, &broken_sha))
            .is_err());
        assert_eq!(manager.load().unwrap(), before);
        assert!(manager
            .root()
            .join(&before.plugins["tpms"].install_path)
            .is_dir());
    }

    #[test]
    fn uninstall_commits_registry_before_payload_cleanup() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager
            .install_vplugin(&package, &expectation(&manifest, &sha256))
            .unwrap();
        let install_path = manager.load().unwrap().plugins["tpms"].install_path.clone();

        let removed = manager.uninstall("tpms").unwrap();
        assert_eq!(removed.manifest.id, "tpms");
        assert!(manager.load().unwrap().plugins.is_empty());
        assert!(!manager.root().join(install_path).exists());
    }
}
