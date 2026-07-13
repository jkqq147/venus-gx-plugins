use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{
    contract::is_sha256, error::io_error, package::validate_payload, CoreError, PluginManifest,
};

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.schema != REGISTRY_SCHEMA_VERSION {
            return Err(CoreError::InvalidRegistry(format!(
                "unsupported schema version {}",
                self.schema
            )));
        }

        for (id, plugin) in &self.plugins {
            plugin
                .manifest
                .validate()
                .map_err(|error| CoreError::InvalidRegistry(error.to_string()))?;
            if id != &plugin.manifest.id {
                return Err(CoreError::InvalidRegistry(format!(
                    "registry key {id} does not match manifest id {}",
                    plugin.manifest.id
                )));
            }
            if !is_sha256(&plugin.package_sha256) {
                return Err(CoreError::InvalidRegistry(format!(
                    "invalid SHA-256 for plugin {id}"
                )));
            }

            let expected_path = install_path(id, &plugin.package_sha256);
            if plugin.install_path != expected_path {
                return Err(CoreError::InvalidRegistry(format!(
                    "invalid install path for plugin {id}: {}",
                    plugin.install_path
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub package_sha256: String,
    pub install_path: String,
}

#[derive(Debug, Clone)]
pub struct LocalRegistry {
    pub(crate) root: PathBuf,
}

impl LocalRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn initialize(&self) -> Result<PluginRegistry, CoreError> {
        self.with_exclusive_lock(|| {
            let registry = self.read_registry_for_update_locked()?;
            if !self.registry_path().exists() {
                self.write_registry_atomic(&registry)?;
            }
            Ok(registry)
        })
    }

    pub fn load(&self) -> Result<PluginRegistry, CoreError> {
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

    pub(crate) fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
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

    fn prepare_root(&self) -> Result<(), CoreError> {
        create_dir_all(&self.root)?;
        require_directory(&self.root)?;
        let plugins = self.root.join("plugins");
        create_dir_all(&plugins)?;
        require_directory(&plugins)?;
        let staging = self.root.join("staging");
        create_dir_all(&staging)?;
        require_directory(&staging)
    }

    fn cleanup_staging_locked(&self) -> Result<(), CoreError> {
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

    fn open_lock(&self) -> Result<File, CoreError> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.lock_path())
            .map_err(|source| io_error(self.lock_path(), source))
    }

    fn read_registry_locked(&self) -> Result<PluginRegistry, CoreError> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(PluginRegistry::default());
        }
        let contents = fs::read(&path).map_err(|source| io_error(path.clone(), source))?;
        let registry: PluginRegistry = serde_json::from_slice(&contents)
            .map_err(|error| CoreError::InvalidRegistry(error.to_string()))?;
        registry.validate()?;
        self.validate_installed_payloads(&registry)?;
        Ok(registry)
    }

    pub(crate) fn read_registry_for_update_locked(&self) -> Result<PluginRegistry, CoreError> {
        let registry = self.read_registry_locked()?;
        self.cleanup_unreferenced_payloads_locked(&registry)?;
        Ok(registry)
    }

    fn cleanup_unreferenced_payloads_locked(
        &self,
        registry: &PluginRegistry,
    ) -> Result<(), CoreError> {
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

    fn validate_installed_payloads(&self, registry: &PluginRegistry) -> Result<(), CoreError> {
        for plugin in registry.plugins.values() {
            let payload = self.root.join(&plugin.install_path);
            let plugin_directory = payload.parent().ok_or_else(|| {
                CoreError::InvalidRegistry(format!(
                    "installed payload for {} has no parent directory",
                    plugin.manifest.id
                ))
            })?;
            require_directory(plugin_directory).map_err(|error| {
                CoreError::InvalidRegistry(format!(
                    "installed directory for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            require_directory(&payload).map_err(|error| {
                CoreError::InvalidRegistry(format!(
                    "installed payload for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            validate_payload(&payload, &plugin.manifest).map_err(|error| {
                CoreError::InvalidRegistry(format!(
                    "installed payload for {} is invalid: {error}",
                    plugin.manifest.id
                ))
            })?;
            let manifest_path = payload.join("manifest.json");
            let contents = fs::read(&manifest_path)
                .map_err(|source| io_error(manifest_path.clone(), source))?;
            let disk_manifest: PluginManifest =
                serde_json::from_slice(&contents).map_err(|error| {
                    CoreError::InvalidRegistry(format!("{}: {error}", manifest_path.display()))
                })?;
            if disk_manifest != plugin.manifest {
                return Err(CoreError::InvalidRegistry(format!(
                    "installed manifest for {} differs from registry",
                    plugin.manifest.id
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn write_registry_atomic(&self, registry: &PluginRegistry) -> Result<(), CoreError> {
        registry.validate()?;
        let mut contents = serde_json::to_vec_pretty(registry)
            .map_err(|error| CoreError::InvalidRegistry(error.to_string()))?;
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
            // rename 是事务提交点；提交后的目录 fsync 失败不能再回滚新 payload。
            let _ = File::open(&self.root).and_then(|directory| directory.sync_all());
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    fn create_registry_temp_file(&self) -> Result<(PathBuf, File), CoreError> {
        for _ in 0..100 {
            let suffix = next_temp_suffix();
            let path = self.root.join(format!(".registry.json.tmp-{suffix}"));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(path, source)),
            }
        }
        Err(CoreError::InvalidRegistry(
            "could not allocate a registry temporary file".into(),
        ))
    }

    pub(crate) fn create_transaction_dir(&self) -> Result<PathBuf, CoreError> {
        for _ in 0..100 {
            let path = self
                .root
                .join("staging")
                .join(format!("transaction-{}", next_temp_suffix()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(io_error(path, source)),
            }
        }
        Err(CoreError::InvalidPackage(
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

pub(crate) fn install_path(id: &str, sha256: &str) -> String {
    format!("plugins/{id}/{}", sha256.to_ascii_lowercase())
}

pub(crate) fn create_dir_all(path: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(path).map_err(|error| io_error(path.to_path_buf(), error))
}

pub(crate) fn remove_dir_all(path: &Path) -> Result<(), CoreError> {
    fs::remove_dir_all(path).map_err(|error| io_error(path.to_path_buf(), error))
}

pub(crate) fn require_directory(path: &Path) -> Result<(), CoreError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| CoreError::InvalidRegistry(format!("{}: {error}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(CoreError::InvalidRegistry(format!(
            "required path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn prune_empty_plugin_dir(root: &Path, id: &str) {
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
