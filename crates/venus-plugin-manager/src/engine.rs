use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use plugin_manager_core::{
    lifecycle_state, plan_reconciliation, Catalog, CoreError, DesiredPluginState, InstallOutcome,
    InstalledPlugin, LifecycleState, LocalRegistry, ObservedPluginState, PackageExpectation,
    ReconcileAction, ServiceState,
};
use thiserror::Error;

use crate::{
    catalog::{CatalogClient, CatalogError, HttpTransport},
    runit::{PluginRuntime, RuntimeError},
    settings::{SettingsError, SettingsStore},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSnapshot {
    pub id: String,
    pub path_key: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
    pub available: bool,
    pub enabled: bool,
    pub installed_version: String,
    pub catalog_version: String,
    pub has_update: bool,
    pub service_state: ServiceState,
    pub lifecycle: LifecycleState,
    pub error: String,
    pub settings_page: String,
    pub dashboard_component: String,
    pub device_list_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSnapshot {
    pub catalog_loaded: bool,
    pub plugins: Vec<PluginSnapshot>,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Registry(#[from] CoreError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("plugin is not installed: {0}")]
    NotInstalled(String),
}

#[derive(Debug, Clone)]
struct ReconciledState {
    enabled: bool,
    service: ServiceState,
    lifecycle: LifecycleState,
    error: String,
}

pub struct ManagerEngine<S, R, T> {
    registry: LocalRegistry,
    settings: S,
    runtime: R,
    catalog_client: CatalogClient<T>,
    catalog: Catalog,
    catalog_loaded: bool,
    reconciled: BTreeMap<String, ReconciledState>,
}

impl<S: SettingsStore, R: PluginRuntime, T: HttpTransport> ManagerEngine<S, R, T> {
    pub fn new(
        registry: LocalRegistry,
        settings: S,
        runtime: R,
        catalog_client: CatalogClient<T>,
    ) -> Self {
        Self {
            registry,
            settings,
            runtime,
            catalog_client,
            catalog: Catalog {
                schema: plugin_manager_core::CATALOG_SCHEMA_VERSION,
                plugins: Vec::new(),
            },
            catalog_loaded: false,
            reconciled: BTreeMap::new(),
        }
    }

    pub fn initialize(&mut self) -> Result<ManagerSnapshot, EngineError> {
        let registry = self.registry.initialize()?;
        for plugin in registry.plugins.values() {
            self.settings.ensure_enabled(&plugin.manifest)?;
        }
        self.reconcile_all()?;
        self.snapshot()
    }

    pub fn refresh_catalog(&mut self) -> Result<ManagerSnapshot, EngineError> {
        match self.catalog_client.refresh() {
            Ok(catalog) => {
                self.catalog = catalog;
                self.catalog_loaded = true;
            }
            Err(error) => {
                return Err(error.into());
            }
        }
        self.snapshot()
    }

    pub fn install(&mut self, id: &str) -> Result<InstallOutcome, EngineError> {
        let before = self.registry.load()?.plugins.get(id).cloned();
        let was_enabled = before
            .as_ref()
            .map(|plugin| self.settings.ensure_enabled(&plugin.manifest))
            .transpose()?
            .unwrap_or(false);
        let (package, entry) = match self.catalog_client.download_plugin(&self.catalog, id) {
            Ok(download) => download,
            Err(error) => {
                self.restore_previous(&before, was_enabled);
                return Err(error.into());
            }
        };
        if was_enabled {
            if let Some(plugin) = &before {
                if let Err(error) = self.runtime.stop(plugin) {
                    let _ = fs::remove_file(&package);
                    return Err(error.into());
                }
            }
        }
        let expectation = PackageExpectation {
            id: entry.id,
            version: entry.version,
            sha256: entry.package.sha256,
        };
        let result = self.registry.install_vplugin(&package, &expectation);
        let _ = fs::remove_file(&package);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.restore_previous(&before, was_enabled);
                return Err(error.into());
            }
        };

        let installed = self
            .registry
            .load()?
            .plugins
            .get(id)
            .cloned()
            .ok_or_else(|| EngineError::NotInstalled(id.to_owned()))?;
        self.settings.ensure_enabled(&installed.manifest)?;
        self.runtime.sync_definition(&installed)?;
        if was_enabled {
            self.settings.write_enabled(&installed.manifest, true)?;
            self.runtime.start(&installed)?;
        }
        self.reconcile_one(&installed);
        Ok(outcome)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), EngineError> {
        let plugin = self
            .registry
            .load()?
            .plugins
            .get(id)
            .cloned()
            .ok_or_else(|| EngineError::NotInstalled(id.to_owned()))?;
        self.settings.ensure_enabled(&plugin.manifest)?;
        self.settings.write_enabled(&plugin.manifest, enabled)?;
        self.reconcile_one(&plugin);
        if let Some(state) = self.reconciled.get(id) {
            if !state.error.is_empty() {
                return Err(EngineError::Runtime(RuntimeError::Command {
                    command: "reconcile".into(),
                    path: PathBuf::from(id),
                    message: state.error.clone(),
                }));
            }
        }
        Ok(())
    }

    pub fn uninstall(&mut self, id: &str, purge_config: bool) -> Result<(), EngineError> {
        let plugin = self
            .registry
            .load()?
            .plugins
            .get(id)
            .cloned()
            .ok_or_else(|| EngineError::NotInstalled(id.to_owned()))?;
        self.settings.ensure_enabled(&plugin.manifest)?;
        self.settings.write_enabled(&plugin.manifest, false)?;
        self.runtime.stop(&plugin)?;
        self.runtime.remove_definition(&plugin)?;
        self.registry.uninstall(id)?;
        self.settings.remove_enabled(&plugin.manifest)?;
        if purge_config {
            self.runtime.purge_config(&plugin)?;
        }
        self.reconciled.remove(id);
        Ok(())
    }

    pub fn reconcile_all(&mut self) -> Result<(), EngineError> {
        let registry = self.registry.load()?;
        for plugin in registry.plugins.values() {
            self.reconcile_one(plugin);
        }
        self.reconciled
            .retain(|id, _| registry.plugins.contains_key(id));
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<ManagerSnapshot, EngineError> {
        let registry = self.registry.load()?;
        let catalog_by_id: BTreeMap<_, _> = self
            .catalog
            .plugins
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        let ids: BTreeSet<String> = registry
            .plugins
            .keys()
            .chain(self.catalog.plugins.iter().map(|entry| &entry.id))
            .cloned()
            .collect();

        let mut plugins = Vec::with_capacity(ids.len());
        for id in ids {
            let installed = registry.plugins.get(&id);
            let catalog = catalog_by_id.get(id.as_str()).copied();
            let state = self.reconciled.get(&id);
            let installed_version = installed
                .map(|plugin| plugin.manifest.version.clone())
                .unwrap_or_default();
            let catalog_version = catalog
                .map(|entry| entry.version.clone())
                .unwrap_or_default();
            let error = state.map(|state| state.error.clone()).unwrap_or_default();
            let lifecycle = state
                .map(|state| state.lifecycle)
                .unwrap_or(LifecycleState::Disabled);
            let payload_root =
                installed.map(|plugin| self.registry.root().join(&plugin.install_path));
            plugins.push(PluginSnapshot {
                id: id.clone(),
                path_key: id.replace('-', "_"),
                name: catalog
                    .map(|entry| entry.name.clone())
                    .or_else(|| installed.map(|plugin| plugin.manifest.name.clone()))
                    .unwrap_or_else(|| id.clone()),
                description: catalog
                    .map(|entry| entry.description.clone())
                    .or_else(|| installed.map(|plugin| plugin.manifest.description.clone()))
                    .unwrap_or_default(),
                installed: installed.is_some(),
                available: catalog.is_some(),
                enabled: state.is_some_and(|state| state.enabled),
                installed_version: installed_version.clone(),
                catalog_version: catalog_version.clone(),
                has_update: installed.is_some()
                    && catalog.is_some()
                    && installed_version != catalog_version,
                service_state: state
                    .map(|state| state.service)
                    .unwrap_or(ServiceState::NotApplicable),
                lifecycle,
                error,
                settings_page: installed
                    .and_then(|plugin| plugin.manifest.ui.settings_page.as_ref())
                    .zip(payload_root.as_ref())
                    .map(|(path, root)| root.join(path).to_string_lossy().into_owned())
                    .unwrap_or_default(),
                dashboard_component: installed
                    .and_then(|plugin| plugin.manifest.ui.dashboard_component.as_ref())
                    .zip(payload_root.as_ref())
                    .map(|(path, root)| root.join(path).to_string_lossy().into_owned())
                    .unwrap_or_default(),
                device_list_values: installed
                    .and_then(|plugin| plugin.manifest.ui.device_list.as_ref())
                    .map(|device_list| device_list.value_paths.clone())
                    .unwrap_or_default(),
            });
        }
        Ok(ManagerSnapshot {
            catalog_loaded: self.catalog_loaded,
            plugins,
        })
    }

    fn reconcile_one(&mut self, plugin: &InstalledPlugin) {
        let result = self.reconcile_plugin(plugin);
        let state = match result {
            Ok(state) => state,
            Err(error) => ReconciledState {
                enabled: self
                    .settings
                    .read_enabled(&plugin.manifest)
                    .unwrap_or(false),
                service: self.runtime.observe(plugin).unwrap_or(ServiceState::Failed),
                lifecycle: LifecycleState::Degraded,
                error: error.to_string(),
            },
        };
        self.reconciled.insert(plugin.manifest.id.clone(), state);
    }

    fn reconcile_plugin(&self, plugin: &InstalledPlugin) -> Result<ReconciledState, EngineError> {
        let enabled = self.settings.read_enabled(&plugin.manifest)?;
        self.runtime.sync_definition(plugin)?;
        let service = self.runtime.observe(plugin)?;
        let ui_visible = enabled && !plugin.manifest.ui.is_empty();
        let desired = DesiredPluginState { enabled };
        let observed = ObservedPluginState {
            service,
            ui_visible,
        };
        for action in plan_reconciliation(&plugin.manifest, desired, observed) {
            match action {
                ReconcileAction::StartService => self.runtime.start(plugin)?,
                ReconcileAction::StopService => self.runtime.stop(plugin)?,
                ReconcileAction::ShowUi | ReconcileAction::HideUi => {
                    // QML visibility is a direct binding to the Enabled setting.
                }
            }
        }
        let service = self.runtime.observe(plugin)?;
        let observed = ObservedPluginState {
            service,
            ui_visible,
        };
        Ok(ReconciledState {
            enabled,
            service,
            lifecycle: lifecycle_state(&plugin.manifest, desired, observed),
            error: String::new(),
        })
    }

    fn restore_previous(&self, previous: &Option<InstalledPlugin>, was_enabled: bool) {
        let Some(plugin) = previous else {
            return;
        };
        let _ = self.runtime.sync_definition(plugin);
        if was_enabled {
            let _ = self.runtime.start(plugin);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        io::Write,
        path::Path,
        sync::Mutex,
    };

    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use flate2::{write::GzEncoder, Compression};
    use plugin_manager_core::{
        CatalogEntry, PackageSource, PluginManifest, PluginSettings, PluginUi, Runtime,
        CATALOG_SCHEMA_VERSION, MANIFEST_SCHEMA_VERSION,
    };
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeSettings {
        values: Mutex<HashMap<String, bool>>,
    }

    impl SettingsStore for FakeSettings {
        fn ensure_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError> {
            let mut values = self.values.lock().unwrap();
            Ok(*values.entry(manifest.id.clone()).or_insert(false))
        }

        fn read_enabled(&self, manifest: &PluginManifest) -> Result<bool, SettingsError> {
            Ok(*self
                .values
                .lock()
                .unwrap()
                .get(&manifest.id)
                .unwrap_or(&false))
        }

        fn write_enabled(
            &self,
            manifest: &PluginManifest,
            enabled: bool,
        ) -> Result<(), SettingsError> {
            self.values
                .lock()
                .unwrap()
                .insert(manifest.id.clone(), enabled);
            Ok(())
        }

        fn remove_enabled(&self, manifest: &PluginManifest) -> Result<(), SettingsError> {
            self.values.lock().unwrap().remove(&manifest.id);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeRuntime {
        running: Mutex<HashMap<String, bool>>,
        configs: Mutex<HashSet<String>>,
        sync_count: Mutex<usize>,
    }

    impl PluginRuntime for FakeRuntime {
        fn sync_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
            *self.sync_count.lock().unwrap() += 1;
            self.configs
                .lock()
                .unwrap()
                .insert(plugin.manifest.id.clone());
            Ok(())
        }

        fn remove_definition(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
            self.running.lock().unwrap().remove(&plugin.manifest.id);
            Ok(())
        }

        fn purge_config(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
            self.configs.lock().unwrap().remove(&plugin.manifest.id);
            Ok(())
        }

        fn observe(&self, plugin: &InstalledPlugin) -> Result<ServiceState, RuntimeError> {
            if matches!(plugin.manifest.runtime, Runtime::QmlOnly) {
                return Ok(ServiceState::NotApplicable);
            }
            Ok(
                if self
                    .running
                    .lock()
                    .unwrap()
                    .get(&plugin.manifest.id)
                    .copied()
                    .unwrap_or(false)
                {
                    ServiceState::Running
                } else {
                    ServiceState::Stopped
                },
            )
        }

        fn start(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
            self.running
                .lock()
                .unwrap()
                .insert(plugin.manifest.id.clone(), true);
            Ok(())
        }

        fn stop(&self, plugin: &InstalledPlugin) -> Result<(), RuntimeError> {
            self.running
                .lock()
                .unwrap()
                .insert(plugin.manifest.id.clone(), false);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeTransport {
        responses: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl HttpTransport for FakeTransport {
        fn download(
            &self,
            url: &str,
            destination: &mut dyn Write,
            limit: u64,
        ) -> Result<u64, CatalogError> {
            let contents = self
                .responses
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| CatalogError::Http {
                    url: url.into(),
                    message: "offline".into(),
                })?;
            if contents.len() as u64 > limit {
                return Err(CatalogError::TooLarge {
                    url: url.into(),
                    limit,
                });
            }
            destination.write_all(&contents).unwrap();
            Ok(contents.len() as u64)
        }
    }

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            description: "Bluetooth tire pressure monitoring".into(),
            version: "0.1.0".into(),
            runtime: Runtime::NativeService {
                executable: "bin/tpms".into(),
                arguments: Vec::new(),
            },
            settings: PluginSettings {
                enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
            },
            ui: PluginUi {
                settings_page: Some("qml/PageTpms.qml".into()),
                dashboard_component: None,
                device_list: None,
            },
        }
    }

    fn package(directory: &Path, manifest: &PluginManifest) -> (Vec<u8>, String) {
        let path = directory.join("tpms.vplugin");
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, contents) in [
            ("manifest.json", serde_json::to_vec(manifest).unwrap()),
            ("bin/tpms", b"binary".to_vec()),
            ("qml/PageTpms.qml", b"Item {}".to_vec()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, contents.as_slice())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        let bytes = fs::read(path).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        (bytes, digest)
    }

    fn signed_package_source(url: &str, sha256: &str) -> PackageSource {
        let key = SigningKey::from_bytes(&[7; 32]);
        let signature = key.sign(&crate::signing::signature_message_parts(
            "tpms", "0.1.0", sha256,
        ));
        PackageSource {
            url: url.into(),
            sha256: sha256.into(),
            signature: plugin_manager_core::PackageSignature {
                key_id: "test-key".into(),
                ed25519: STANDARD.encode(signature.to_bytes()),
            },
        }
    }

    fn verifier() -> crate::signing::CatalogVerifier {
        let public = SigningKey::from_bytes(&[7; 32]).verifying_key();
        crate::signing::CatalogVerifier::from_base64(
            "test-key",
            &STANDARD.encode(public.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn installs_enables_and_uninstalls_complete_lifecycle() {
        let temp = TempDir::new().unwrap();
        let catalog_url = "https://example.com/plugins.json";
        let package_url = "https://example.com/tpms.vplugin";
        let manifest = manifest();
        let (package, sha256) = package(temp.path(), &manifest);
        let catalog = Catalog {
            schema: CATALOG_SCHEMA_VERSION,
            plugins: vec![CatalogEntry {
                id: "tpms".into(),
                name: "TPMS".into(),
                description: "Bluetooth tire pressure monitoring".into(),
                version: "0.1.0".into(),
                package: signed_package_source(package_url, &sha256),
            }],
        };
        let transport = FakeTransport::default();
        transport
            .responses
            .lock()
            .unwrap()
            .insert(catalog_url.into(), serde_json::to_vec(&catalog).unwrap());
        transport
            .responses
            .lock()
            .unwrap()
            .insert(package_url.into(), package);
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );
        let mut engine = ManagerEngine::new(
            LocalRegistry::new(temp.path().join("state")),
            FakeSettings::default(),
            FakeRuntime::default(),
            client,
        );

        assert!(engine.initialize().unwrap().plugins.is_empty());
        assert_eq!(engine.refresh_catalog().unwrap().plugins.len(), 1);
        assert_eq!(engine.install("tpms").unwrap(), InstallOutcome::Installed);
        let sync_count = *engine.runtime.sync_count.lock().unwrap();
        let installed = engine.snapshot().unwrap().plugins.remove(0);
        assert!(installed.installed);
        assert!(!installed.enabled);
        assert_eq!(*engine.runtime.sync_count.lock().unwrap(), sync_count);

        engine.set_enabled("tpms", true).unwrap();
        let enabled = engine.snapshot().unwrap().plugins.remove(0);
        assert!(enabled.enabled);
        assert_eq!(enabled.lifecycle, LifecycleState::Enabled);

        engine.uninstall("tpms", false).unwrap();
        assert!(engine.runtime.configs.lock().unwrap().contains("tpms"));
        let available = engine.snapshot().unwrap().plugins.remove(0);
        assert!(!available.installed);
        assert!(available.available);

        assert_eq!(engine.install("tpms").unwrap(), InstallOutcome::Installed);
        engine.uninstall("tpms", true).unwrap();
        assert!(!engine.runtime.configs.lock().unwrap().contains("tpms"));
    }
}
