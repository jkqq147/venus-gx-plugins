use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::read::GzDecoder;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{is_sha256, PluginManifest, Runtime};

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 512;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRegistry {
    pub schema: u32,
    pub plugins: BTreeMap<String, InstalledPlugin>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self {
            schema: REGISTRY_SCHEMA_VERSION,
            plugins: BTreeMap::new(),
        }
    }
}

impl PluginRegistry {
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.schema != REGISTRY_SCHEMA_VERSION {
            return Err(RegistryError::InvalidRegistry(format!(
                "unsupported schema version {}",
                self.schema
            )));
        }

        for (id, plugin) in &self.plugins {
            plugin
                .manifest
                .validate()
                .map_err(|error| RegistryError::InvalidRegistry(error.to_string()))?;
            if id != &plugin.manifest.id {
                return Err(RegistryError::InvalidRegistry(format!(
                    "registry key {id} does not match manifest id {}",
                    plugin.manifest.id
                )));
            }
            if !is_sha256(&plugin.package_sha256) {
                return Err(RegistryError::InvalidRegistry(format!(
                    "invalid SHA-256 for plugin {id}"
                )));
            }

            let expected_path = install_path(id, &plugin.package_sha256);
            if plugin.install_path != expected_path {
                return Err(RegistryError::InvalidRegistry(format!(
                    "invalid install path for plugin {id}: {}",
                    plugin.install_path
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub enabled: bool,
    pub package_sha256: String,
    pub install_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageExpectation {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Updated,
    Unchanged,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid registry: {0}")]
    InvalidRegistry(String),
    #[error("invalid plugin package: {0}")]
    InvalidPackage(String),
    #[error("package exceeds the {MAX_PACKAGE_BYTES} byte size limit")]
    PackageTooLarge,
    #[error("package checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("package {field} mismatch: expected {expected}, got {actual}")]
    IdentityMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("plugin is not installed: {0}")]
    NotInstalled(String),
}

#[derive(Debug, Clone)]
pub struct LocalRegistry {
    root: PathBuf,
}

impl LocalRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn initialize(&self) -> Result<PluginRegistry, RegistryError> {
        self.with_exclusive_lock(|| {
            let registry = self.read_registry_for_update_locked()?;
            if !self.registry_path().exists() {
                self.write_registry_atomic(&registry)?;
            }
            Ok(registry)
        })
    }

    pub fn load(&self) -> Result<PluginRegistry, RegistryError> {
        self.prepare_root()?;
        let lock = self.open_lock()?;
        lock.lock_shared()
            .map_err(|source| io_error(self.lock_path(), source))?;
        let result = self.read_registry_locked();
        let unlock_result =
            FileExt::unlock(&lock).map_err(|source| io_error(self.lock_path(), source));
        match (result, unlock_result) {
            (Ok(registry), Ok(())) => Ok(registry),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    pub fn install_vplugin(
        &self,
        package_path: &Path,
        expectation: &PackageExpectation,
    ) -> Result<InstallOutcome, RegistryError> {
        if !is_sha256(&expectation.sha256) {
            return Err(RegistryError::InvalidPackage(
                "expected SHA-256 must contain exactly 64 hexadecimal characters".into(),
            ));
        }
        let expected_sha256 = expectation.sha256.to_ascii_lowercase();

        self.with_exclusive_lock(|| {
            let mut registry = self.read_registry_for_update_locked()?;
            let transaction_dir = self.create_transaction_dir()?;
            let result = (|| {
                let copied_package = transaction_dir.join("package.vplugin");
                let actual_sha256 = copy_and_hash(package_path, &copied_package)?;
                if actual_sha256 != expected_sha256 {
                    return Err(RegistryError::ChecksumMismatch {
                        expected: expected_sha256.clone(),
                        actual: actual_sha256,
                    });
                }

                let payload = transaction_dir.join("payload");
                let manifest = extract_package(&copied_package, &payload)?;
                if manifest.id != expectation.id {
                    return Err(RegistryError::IdentityMismatch {
                        field: "id",
                        expected: expectation.id.clone(),
                        actual: manifest.id,
                    });
                }
                if manifest.version != expectation.version {
                    return Err(RegistryError::IdentityMismatch {
                        field: "version",
                        expected: expectation.version.clone(),
                        actual: manifest.version,
                    });
                }

                let previous = registry.plugins.get(&manifest.id).cloned();
                if previous.as_ref().is_some_and(|plugin| {
                    plugin.package_sha256 == expected_sha256 && plugin.manifest == manifest
                }) {
                    return Ok(InstallOutcome::Unchanged);
                }

                let relative_install_path = install_path(&manifest.id, &expected_sha256);
                let final_path = self.root.join(&relative_install_path);
                if previous
                    .as_ref()
                    .is_some_and(|plugin| plugin.install_path == relative_install_path)
                {
                    return Err(RegistryError::InvalidPackage(
                        "package digest matches the installed payload but its manifest differs"
                            .into(),
                    ));
                }
                if final_path.exists() {
                    remove_dir_all(&final_path)?;
                }
                let parent = final_path.parent().ok_or_else(|| {
                    RegistryError::InvalidRegistry("install path has no parent".into())
                })?;
                create_dir_all(parent)?;
                require_directory(parent)?;
                fs::rename(&payload, &final_path)
                    .map_err(|source| io_error(final_path.clone(), source))?;

                let enabled = previous.as_ref().is_some_and(|plugin| plugin.enabled);
                registry.plugins.insert(
                    manifest.id.clone(),
                    InstalledPlugin {
                        manifest: manifest.clone(),
                        enabled,
                        package_sha256: expected_sha256.clone(),
                        install_path: relative_install_path,
                    },
                );

                if let Err(error) = self.write_registry_atomic(&registry) {
                    let _ = fs::remove_dir_all(&final_path);
                    return Err(error);
                }

                if let Some(previous) = &previous {
                    if previous.install_path != registry.plugins[&manifest.id].install_path {
                        let _ = fs::remove_dir_all(self.root.join(&previous.install_path));
                    }
                }
                prune_empty_plugin_dir(&self.root, &manifest.id);

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

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<InstalledPlugin, RegistryError> {
        self.with_exclusive_lock(|| {
            let mut registry = self.read_registry_for_update_locked()?;
            let plugin = registry
                .plugins
                .get_mut(id)
                .ok_or_else(|| RegistryError::NotInstalled(id.to_owned()))?;
            if plugin.enabled != enabled {
                plugin.enabled = enabled;
                self.write_registry_atomic(&registry)?;
            }
            Ok(registry.plugins[id].clone())
        })
    }

    pub fn uninstall(&self, id: &str) -> Result<InstalledPlugin, RegistryError> {
        self.with_exclusive_lock(|| {
            let mut registry = self.read_registry_for_update_locked()?;
            let removed = registry
                .plugins
                .remove(id)
                .ok_or_else(|| RegistryError::NotInstalled(id.to_owned()))?;
            self.write_registry_atomic(&registry)?;

            let _ = fs::remove_dir_all(self.root.join(&removed.install_path));
            prune_empty_plugin_dir(&self.root, id);
            Ok(removed)
        })
    }

    fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        self.prepare_root()?;
        let lock = self.open_lock()?;
        lock.lock_exclusive()
            .map_err(|source| io_error(self.lock_path(), source))?;
        let result = self.cleanup_staging_locked().and_then(|()| operation());
        let unlock_result =
            FileExt::unlock(&lock).map_err(|source| io_error(self.lock_path(), source));
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn prepare_root(&self) -> Result<(), RegistryError> {
        create_dir_all(&self.root)?;
        require_directory(&self.root)?;
        let plugins = self.root.join("plugins");
        create_dir_all(&plugins)?;
        require_directory(&plugins)?;
        let staging = self.root.join("staging");
        create_dir_all(&staging)?;
        require_directory(&staging)
    }

    fn cleanup_staging_locked(&self) -> Result<(), RegistryError> {
        let staging = self.root.join("staging");
        for entry in fs::read_dir(&staging).map_err(|source| io_error(staging.clone(), source))? {
            let entry = entry.map_err(|source| io_error(staging.clone(), source))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|source| io_error(path.clone(), source))?;
            if file_type.is_dir() {
                remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path).map_err(|source| io_error(path, source))?;
            }
        }
        for entry in
            fs::read_dir(&self.root).map_err(|source| io_error(self.root.clone(), source))?
        {
            let entry = entry.map_err(|source| io_error(self.root.clone(), source))?;
            let name = entry.file_name();
            if !name
                .to_str()
                .is_some_and(|name| name.starts_with(".registry.json.tmp-"))
            {
                continue;
            }
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|source| io_error(path.clone(), source))?;
            if file_type.is_dir() {
                remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path).map_err(|source| io_error(path, source))?;
            }
        }
        Ok(())
    }

    fn open_lock(&self) -> Result<File, RegistryError> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.lock_path())
            .map_err(|source| io_error(self.lock_path(), source))
    }

    fn read_registry_locked(&self) -> Result<PluginRegistry, RegistryError> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(PluginRegistry::default());
        }
        let contents = fs::read(&path).map_err(|source| io_error(path.clone(), source))?;
        let registry: PluginRegistry = serde_json::from_slice(&contents)
            .map_err(|error| RegistryError::InvalidRegistry(error.to_string()))?;
        registry.validate()?;
        self.validate_installed_payloads(&registry)?;
        Ok(registry)
    }

    fn read_registry_for_update_locked(&self) -> Result<PluginRegistry, RegistryError> {
        let registry = self.read_registry_locked()?;
        self.cleanup_unreferenced_payloads_locked(&registry)?;
        Ok(registry)
    }

    fn cleanup_unreferenced_payloads_locked(
        &self,
        registry: &PluginRegistry,
    ) -> Result<(), RegistryError> {
        let referenced: HashSet<PathBuf> = registry
            .plugins
            .values()
            .map(|plugin| self.root.join(&plugin.install_path))
            .collect();
        let plugins_root = self.root.join("plugins");

        for id_entry in
            fs::read_dir(&plugins_root).map_err(|source| io_error(plugins_root.clone(), source))?
        {
            let id_entry = id_entry.map_err(|source| io_error(plugins_root.clone(), source))?;
            let id_path = id_entry.path();
            let id_type = id_entry
                .file_type()
                .map_err(|source| io_error(id_path.clone(), source))?;
            if !id_type.is_dir() {
                fs::remove_file(&id_path).map_err(|source| io_error(id_path, source))?;
                continue;
            }

            for payload_entry in
                fs::read_dir(&id_path).map_err(|source| io_error(id_path.clone(), source))?
            {
                let payload_entry =
                    payload_entry.map_err(|source| io_error(id_path.clone(), source))?;
                let payload_path = payload_entry.path();
                if referenced.contains(&payload_path) {
                    continue;
                }
                let payload_type = payload_entry
                    .file_type()
                    .map_err(|source| io_error(payload_path.clone(), source))?;
                if payload_type.is_dir() {
                    remove_dir_all(&payload_path)?;
                } else {
                    fs::remove_file(&payload_path)
                        .map_err(|source| io_error(payload_path, source))?;
                }
            }
            let _ = fs::remove_dir(&id_path);
        }
        Ok(())
    }

    fn validate_installed_payloads(&self, registry: &PluginRegistry) -> Result<(), RegistryError> {
        for plugin in registry.plugins.values() {
            let payload = self.root.join(&plugin.install_path);
            let plugin_directory = payload.parent().ok_or_else(|| {
                RegistryError::InvalidRegistry(format!(
                    "installed payload for {} has no parent directory",
                    plugin.manifest.id
                ))
            })?;
            require_directory(plugin_directory).map_err(|error| {
                RegistryError::InvalidRegistry(format!(
                    "installed directory for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            require_directory(&payload).map_err(|error| {
                RegistryError::InvalidRegistry(format!(
                    "installed payload for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            validate_payload(&payload, &plugin.manifest).map_err(|error| {
                RegistryError::InvalidRegistry(format!(
                    "installed payload for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            let manifest_path = payload.join("manifest.json");
            let contents = fs::read(&manifest_path)
                .map_err(|source| io_error(manifest_path.clone(), source))?;
            let disk_manifest: PluginManifest =
                serde_json::from_slice(&contents).map_err(|error| {
                    RegistryError::InvalidRegistry(format!("{}: {error}", manifest_path.display()))
                })?;
            if disk_manifest != plugin.manifest {
                return Err(RegistryError::InvalidRegistry(format!(
                    "installed manifest for {} differs from registry",
                    plugin.manifest.id
                )));
            }
        }
        Ok(())
    }

    fn write_registry_atomic(&self, registry: &PluginRegistry) -> Result<(), RegistryError> {
        registry.validate()?;
        let mut contents = serde_json::to_vec_pretty(registry)
            .map_err(|error| RegistryError::InvalidRegistry(error.to_string()))?;
        contents.push(b'\n');

        let (temp_path, mut temp_file) = self.create_registry_temp_file()?;
        let write_result = (|| {
            temp_file
                .write_all(&contents)
                .map_err(|source| io_error(temp_path.clone(), source))?;
            temp_file
                .sync_all()
                .map_err(|source| io_error(temp_path.clone(), source))?;
            fs::rename(&temp_path, self.registry_path())
                .map_err(|source| io_error(self.registry_path(), source))?;
            // rename 是事务提交点。此后即使目录 fsync 在特殊文件系统上不可用，
            // 也不能向调用者报告“未提交”并删除已被新 Registry 引用的 payload。
            let _ = File::open(&self.root).and_then(|directory| directory.sync_all());
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    fn create_registry_temp_file(&self) -> Result<(PathBuf, File), RegistryError> {
        for _ in 0..100 {
            let suffix = next_temp_suffix();
            let path = self.root.join(format!(".registry.json.tmp-{suffix}"));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(path, source)),
            }
        }
        Err(RegistryError::InvalidRegistry(
            "could not allocate a registry temporary file".into(),
        ))
    }

    fn create_transaction_dir(&self) -> Result<PathBuf, RegistryError> {
        for _ in 0..100 {
            let path = self
                .root
                .join("staging")
                .join(format!("transaction-{}", next_temp_suffix()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(path, source)),
            }
        }
        Err(RegistryError::InvalidPackage(
            "could not allocate a transaction directory".into(),
        ))
    }

    fn registry_path(&self) -> PathBuf {
        self.root.join("registry.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join("registry.lock")
    }
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<String, RegistryError> {
    let mut input = File::open(source).map_err(|error| io_error(source.to_path_buf(), error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| io_error(destination.to_path_buf(), error))?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| io_error(source.to_path_buf(), error))?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > MAX_PACKAGE_BYTES {
            return Err(RegistryError::PackageTooLarge);
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| io_error(destination.to_path_buf(), error))?;
    }
    output
        .sync_all()
        .map_err(|error| io_error(destination.to_path_buf(), error))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_package(package: &Path, destination: &Path) -> Result<PluginManifest, RegistryError> {
    create_dir_all(destination)?;
    let file = File::open(package).map_err(|error| io_error(package.to_path_buf(), error))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| RegistryError::InvalidPackage(error.to_string()))?;
    let mut seen = HashSet::new();
    let mut regular_files = HashSet::new();
    let mut total_size = 0_u64;

    for (index, entry) in entries.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(RegistryError::InvalidPackage(format!(
                "archive contains more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let mut entry = entry.map_err(|error| RegistryError::InvalidPackage(error.to_string()))?;
        let path = entry
            .path()
            .map_err(|error| RegistryError::InvalidPackage(error.to_string()))?
            .into_owned();
        validate_archive_path(&path, entry.header().entry_type().is_dir())?;
        if !seen.insert(path.clone()) {
            return Err(RegistryError::InvalidPackage(format!(
                "duplicate archive path: {}",
                path.display()
            )));
        }

        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(RegistryError::InvalidPackage(format!(
                "unsupported archive entry type at {}",
                path.display()
            )));
        }
        if path == Path::new("manifest.json") && !entry_type.is_file() {
            return Err(RegistryError::InvalidPackage(
                "manifest.json must be a regular file".into(),
            ));
        }

        let target = destination.join(&path);
        if entry_type.is_dir() {
            create_dir_all(&target)?;
            continue;
        }
        total_size = total_size
            .checked_add(entry.header().size().map_err(|error| {
                RegistryError::InvalidPackage(format!("{}: {error}", path.display()))
            })?)
            .ok_or_else(|| RegistryError::InvalidPackage("archive size overflow".into()))?;
        if total_size > MAX_EXTRACTED_BYTES {
            return Err(RegistryError::InvalidPackage(format!(
                "archive expands beyond {MAX_EXTRACTED_BYTES} bytes"
            )));
        }
        if let Some(parent) = target.parent() {
            create_dir_all(parent)?;
        }
        entry.unpack(&target).map_err(|error| {
            RegistryError::InvalidPackage(format!("{}: {error}", path.display()))
        })?;
        regular_files.insert(path);
    }

    if !regular_files.contains(Path::new("manifest.json")) {
        return Err(RegistryError::InvalidPackage(
            "archive does not contain manifest.json".into(),
        ));
    }
    let manifest_path = destination.join("manifest.json");
    let manifest_contents =
        fs::read(&manifest_path).map_err(|error| io_error(manifest_path.clone(), error))?;
    let manifest: PluginManifest = serde_json::from_slice(&manifest_contents)
        .map_err(|error| RegistryError::InvalidPackage(format!("manifest.json: {error}")))?;
    manifest
        .validate()
        .map_err(|error| RegistryError::InvalidPackage(error.to_string()))?;
    validate_payload(destination, &manifest)?;
    normalize_permissions(destination, &manifest)?;
    Ok(manifest)
}

fn validate_archive_path(path: &Path, is_directory: bool) -> Result<(), RegistryError> {
    let text = path
        .to_str()
        .ok_or_else(|| RegistryError::InvalidPackage("archive paths must use UTF-8".into()))?;
    if text.is_empty() || text.contains('\\') {
        return Err(RegistryError::InvalidPackage(format!(
            "invalid archive path: {}",
            path.display()
        )));
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(RegistryError::InvalidPackage(format!(
            "unsafe archive path: {}",
            path.display()
        )));
    }

    let mut components = path.components();
    let first = components.next().and_then(|component| match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    });
    match first {
        Some("manifest.json") if components.next().is_none() => Ok(()),
        Some("bin" | "qml") if is_directory || components.next().is_some() => Ok(()),
        _ => Err(RegistryError::InvalidPackage(format!(
            "path is outside the package contract: {}",
            path.display()
        ))),
    }
}

fn validate_payload(root: &Path, manifest: &PluginManifest) -> Result<(), RegistryError> {
    require_regular_file(&root.join("manifest.json"))?;
    if let Runtime::NativeService { executable } = &manifest.runtime {
        require_regular_file(&root.join(executable))?;
    }
    for relative_path in [&manifest.ui.settings_page, &manifest.ui.dashboard_component]
        .into_iter()
        .flatten()
    {
        require_regular_file(&root.join(relative_path))?;
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), RegistryError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        RegistryError::InvalidPackage(format!("required file {}: {error}", path.display()))
    })?;
    if !metadata.file_type().is_file() {
        return Err(RegistryError::InvalidPackage(format!(
            "required path is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), RegistryError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| RegistryError::InvalidRegistry(format!("{}: {error}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(RegistryError::InvalidRegistry(format!(
            "required path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn normalize_permissions(root: &Path, manifest: &PluginManifest) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt;

    fn visit(path: &Path) -> Result<(), RegistryError> {
        for entry in fs::read_dir(path).map_err(|error| io_error(path.to_path_buf(), error))? {
            let entry = entry.map_err(|error| io_error(path.to_path_buf(), error))?;
            let child = entry.path();
            let metadata =
                fs::symlink_metadata(&child).map_err(|error| io_error(child.clone(), error))?;
            if metadata.is_dir() {
                fs::set_permissions(&child, fs::Permissions::from_mode(0o755))
                    .map_err(|error| io_error(child.clone(), error))?;
                visit(&child)?;
            } else if metadata.is_file() {
                fs::set_permissions(&child, fs::Permissions::from_mode(0o644))
                    .map_err(|error| io_error(child.clone(), error))?;
            } else {
                return Err(RegistryError::InvalidPackage(format!(
                    "unsupported extracted path: {}",
                    child.display()
                )));
            }
        }
        Ok(())
    }

    fs::set_permissions(root, fs::Permissions::from_mode(0o755))
        .map_err(|error| io_error(root.to_path_buf(), error))?;
    visit(root)?;
    if let Runtime::NativeService { executable } = &manifest.runtime {
        let path = root.join(executable);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn normalize_permissions(_root: &Path, _manifest: &PluginManifest) -> Result<(), RegistryError> {
    Ok(())
}

fn install_path(id: &str, sha256: &str) -> String {
    format!("plugins/{id}/{}", sha256.to_ascii_lowercase())
}

fn create_dir_all(path: &Path) -> Result<(), RegistryError> {
    fs::create_dir_all(path).map_err(|error| io_error(path.to_path_buf(), error))
}

fn remove_dir_all(path: &Path) -> Result<(), RegistryError> {
    fs::remove_dir_all(path).map_err(|error| io_error(path.to_path_buf(), error))
}

fn prune_empty_plugin_dir(root: &Path, id: &str) {
    let path = root.join("plugins").join(id);
    let is_empty = fs::read_dir(&path)
        .ok()
        .and_then(|mut entries| entries.next().transpose().ok())
        .flatten()
        .is_none();
    if is_empty {
        let _ = fs::remove_dir(path);
    }
}

fn next_temp_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn io_error(path: PathBuf, source: io::Error) -> RegistryError {
    RegistryError::Io { path, source }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use flate2::{write::GzEncoder, Compression};
    use tempfile::TempDir;

    use super::*;
    use crate::{PluginSettings, PluginUi, SCHEMA_VERSION};

    fn native_manifest(version: &str) -> PluginManifest {
        PluginManifest {
            schema: SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            version: version.into(),
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
        let digest = digest(&package);
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
        let digest = digest(&package);
        (package, digest)
    }

    fn digest(path: &Path) -> String {
        let mut file = File::open(path).unwrap();
        let mut hasher = Sha256::new();
        io::copy(&mut file, &mut HashWriter(&mut hasher)).unwrap();
        format!("{:x}", hasher.finalize())
    }

    struct HashWriter<'a>(&'a mut Sha256);

    impl Write for HashWriter<'_> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.update(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
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
    fn installs_a_verified_package_disabled_by_default() {
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
        assert!(!installed.enabled);
        assert_eq!(installed.manifest, manifest);
        let executable = manager
            .root()
            .join(&installed.install_path)
            .join("bin/venus-tpms-ble");
        assert_eq!(
            fs::metadata(executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn upgrade_preserves_enabled_state_and_removes_old_payload() {
        let temp = TempDir::new().unwrap();
        let first_manifest = native_manifest("0.1.0");
        let (first_package, first_sha) = write_package(temp.path(), &first_manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager
            .install_vplugin(&first_package, &expectation(&first_manifest, &first_sha))
            .unwrap();
        manager.set_enabled("tpms", true).unwrap();
        let old_path = manager.load().unwrap().plugins["tpms"].install_path.clone();

        let second_manifest = native_manifest("0.2.0");
        let (second_package, second_sha) = write_package(temp.path(), &second_manifest, true);
        assert_eq!(
            manager
                .install_vplugin(&second_package, &expectation(&second_manifest, &second_sha),)
                .unwrap(),
            InstallOutcome::Updated
        );

        let installed = manager.load().unwrap().plugins.remove("tpms").unwrap();
        assert!(installed.enabled);
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
        assert!(matches!(error, RegistryError::ChecksumMismatch { .. }));
        assert!(manager.load().unwrap().plugins.is_empty());
    }

    #[test]
    fn catalog_identity_mismatch_does_not_install_package() {
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
            RegistryError::IdentityMismatch { field: "id", .. }
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
        assert!(matches!(error, RegistryError::InvalidPackage(_)));
        assert!(manager.load().unwrap().plugins.is_empty());
        assert!(!temp.path().join("state/qml/escape").exists());
    }

    #[test]
    fn reinstalling_identical_package_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        let expected = expectation(&manifest, &sha256);
        manager.install_vplugin(&package, &expected).unwrap();
        manager.set_enabled("tpms", true).unwrap();

        assert_eq!(
            manager.install_vplugin(&package, &expected).unwrap(),
            InstallOutcome::Unchanged
        );
        assert!(manager.load().unwrap().plugins["tpms"].enabled);
    }

    #[test]
    fn next_exclusive_operation_cleans_abandoned_transaction_files() {
        let temp = TempDir::new().unwrap();
        let manifest = native_manifest("0.1.0");
        let (package, sha256) = write_package(temp.path(), &manifest, true);
        let manager = LocalRegistry::new(temp.path().join("state"));
        manager
            .install_vplugin(&package, &expectation(&manifest, &sha256))
            .unwrap();
        let referenced = manager.load().unwrap().plugins["tpms"].install_path.clone();

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
        assert!(manager.root().join(referenced).is_dir());
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
            .install_vplugin(&broken_package, &expectation(&broken_manifest, &broken_sha),)
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
