use std::{
    env,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use plugin_manager_core::LocalRegistry;
use thiserror::Error;
use zbus::blocking::Connection;

use crate::{
    catalog::{CatalogClient, SystemHttpTransport},
    engine::{EngineError, ManagerEngine},
    publisher::{ManagerCommand, ManagerPublisher},
    runit::{RunitRuntime, SystemRunitController},
    settings::VenusSettings,
};

pub const DEFAULT_APP_ROOT: &str = "/data/venus-gx-plugins";
pub const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/jkqq147/venus-gx-plugins/master/catalog/plugins.json";

type SystemEngine =
    ManagerEngine<VenusSettings, RunitRuntime<SystemRunitController>, SystemHttpTransport>;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("D-Bus operation failed: {0}")]
    Dbus(#[from] zbus::Error),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub app_root: PathBuf,
    pub service_root: PathBuf,
    pub catalog_url: String,
}

impl ServiceConfig {
    pub fn from_env() -> Self {
        Self {
            app_root: env::var_os("VENUS_PLUGIN_MANAGER_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_APP_ROOT)),
            service_root: env::var_os("VENUS_PLUGIN_MANAGER_SERVICE_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/service")),
            catalog_url: env::var("VENUS_PLUGIN_MANAGER_CATALOG_URL")
                .unwrap_or_else(|_| DEFAULT_CATALOG_URL.into()),
        }
    }
}

pub fn run(config: ServiceConfig) -> Result<(), ServiceError> {
    let connection = Connection::system()?;
    let state_root = config.app_root.join("state");
    let settings = VenusSettings::new(connection.clone());
    let runtime = RunitRuntime::new(
        &state_root,
        config.app_root.join("config"),
        config.app_root.join("services"),
        &config.service_root,
    );
    let catalog_client = CatalogClient::new(
        config.catalog_url,
        config.app_root.join("cache/catalog.json"),
        config.app_root.join("downloads"),
    );
    let mut engine = ManagerEngine::new(
        LocalRegistry::new(&state_root),
        settings,
        runtime,
        catalog_client,
    );
    let snapshot = engine.initialize()?;
    let (command_sender, command_receiver) = mpsc::channel();
    let mut publisher = ManagerPublisher::new(connection, command_sender)?;
    publisher.publish(&snapshot, false, "")?;

    let mut last_reconcile = Instant::now();
    let mut last_error = String::new();
    loop {
        match command_receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(command) => {
                last_error = execute_command(&mut engine, &mut publisher, command)?;
                last_reconcile = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_reconcile.elapsed() >= Duration::from_secs(5) {
                    let snapshot = engine.snapshot()?;
                    publisher.publish(&snapshot, false, &last_error)?;
                    last_reconcile = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn execute_command(
    engine: &mut SystemEngine,
    publisher: &mut ManagerPublisher,
    command: ManagerCommand,
) -> Result<String, ServiceError> {
    let before = engine.snapshot()?;
    publisher.publish(&before, true, "")?;
    let result = match command {
        ManagerCommand::Refresh => engine.refresh_catalog().map(|_| ()),
        ManagerCommand::Install(id) => engine.install(&id).map(|_| ()),
        ManagerCommand::SetEnabled(id, enabled) => engine.set_enabled(&id, enabled),
        ManagerCommand::Uninstall(id) => engine.uninstall(&id),
    };
    let last_error = result
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    let snapshot = engine.snapshot()?;
    publisher.publish(&snapshot, false, &last_error)?;
    Ok(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_defaults_match_device_layout() {
        let config = ServiceConfig {
            app_root: PathBuf::from(DEFAULT_APP_ROOT),
            service_root: PathBuf::from("/service"),
            catalog_url: DEFAULT_CATALOG_URL.into(),
        };
        assert_eq!(
            config.app_root.join("state"),
            PathBuf::from("/data/venus-gx-plugins/state")
        );
        assert!(config.catalog_url.starts_with("https://"));
    }
}
