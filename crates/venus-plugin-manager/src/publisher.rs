use std::{
    collections::{HashMap, HashSet},
    sync::{mpsc::Sender, Arc, Mutex},
};

use plugin_manager_core::{LifecycleState, ServiceState};
use zbus::{blocking::Connection, interface, zvariant::OwnedValue};

use crate::{
    bus_item::{BusItem, BusItemHandle},
    engine::{ManagerSnapshot, PluginSnapshot},
};

pub const SERVICE_NAME: &str = "com.victronenergy.pluginmanager";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerCommand {
    Refresh,
    Install(String),
    SetEnabled(String, bool),
    Uninstall(String),
}

pub struct ManagerPublisher {
    connection: Connection,
    handles: Arc<Mutex<HashMap<String, BusItemHandle>>>,
    commands: Sender<ManagerCommand>,
}

impl ManagerPublisher {
    pub fn new(connection: Connection, commands: Sender<ManagerCommand>) -> zbus::Result<Self> {
        let mut publisher = Self {
            connection,
            handles: Arc::new(Mutex::new(HashMap::new())),
            commands,
        };

        publisher.connection.object_server().at(
            "/",
            RootBusItem {
                handles: Arc::clone(&publisher.handles),
            },
        )?;

        for (path, value) in [
            ("/Mgmt/ProcessName", "venus-plugin-manager"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "Venus OS system D-Bus"),
            ("/ProductName", "Plugin Manager"),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/CatalogStatus", "尚未刷新"),
            ("/InstalledIds", ""),
            ("/AvailableIds", ""),
            ("/DashboardIds", ""),
            ("/LastError", ""),
        ] {
            publisher.add(path, BusItem::string(value))?;
        }
        for (path, value) in [
            ("/Connected", 1),
            ("/InstalledCount", 0),
            ("/AvailableCount", 0),
            ("/Busy", 0),
        ] {
            publisher.add(path, BusItem::i32(value))?;
        }
        let sender = publisher.commands.clone();
        publisher.add(
            "/Refresh",
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender.send(ManagerCommand::Refresh).map_or(2, |_| 0)
            }),
        )?;

        publisher.connection.request_name(SERVICE_NAME)?;
        Ok(publisher)
    }

    pub fn publish(
        &mut self,
        snapshot: &ManagerSnapshot,
        busy: bool,
        last_error: &str,
    ) -> zbus::Result<()> {
        self.remove_missing_plugins(&snapshot.plugins)?;
        for plugin in &snapshot.plugins {
            self.ensure_plugin(plugin)?;
        }

        let installed: Vec<_> = snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.installed)
            .map(|plugin| plugin.id.as_str())
            .collect();
        let available: Vec<_> = snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.available)
            .map(|plugin| plugin.id.as_str())
            .collect();
        let dashboards: Vec<_> = snapshot
            .plugins
            .iter()
            .filter(|plugin| {
                plugin.installed && plugin.enabled && !plugin.dashboard_component.is_empty()
            })
            .map(|plugin| plugin.id.as_str())
            .collect();
        self.string("/CatalogStatus", &snapshot.catalog_state.text())?;
        self.string("/InstalledIds", &installed.join(","))?;
        self.string("/AvailableIds", &available.join(","))?;
        self.string("/DashboardIds", &dashboards.join(","))?;
        self.string("/LastError", last_error)?;
        self.i32("/InstalledCount", installed.len() as i32)?;
        self.i32("/AvailableCount", available.len() as i32)?;
        self.i32("/Busy", i32::from(busy))?;
        self.i32("/Refresh", 0)?;

        for plugin in &snapshot.plugins {
            self.publish_plugin(plugin)?;
        }
        Ok(())
    }

    fn remove_missing_plugins(&mut self, plugins: &[PluginSnapshot]) -> zbus::Result<()> {
        let active: HashSet<_> = plugins
            .iter()
            .map(|plugin| plugin.path_key.as_str())
            .collect();
        let stale: Vec<_> = self
            .handles
            .lock()
            .expect("D-Bus handles poisoned")
            .keys()
            .filter_map(|path| {
                plugin_key_from_item_path(path)
                    .filter(|key| !active.contains(key))
                    .map(|_| path.clone())
            })
            .collect();

        for path in stale {
            self.connection
                .object_server()
                .remove::<BusItem, _>(path.as_str())?;
            self.handles
                .lock()
                .expect("D-Bus handles poisoned")
                .remove(&path);
        }
        Ok(())
    }

    fn ensure_plugin(&mut self, plugin: &PluginSnapshot) -> zbus::Result<()> {
        let root = plugin_root(plugin);
        if self
            .handles
            .lock()
            .expect("D-Bus handles poisoned")
            .contains_key(&format!("{root}/Id"))
        {
            return Ok(());
        }
        for suffix in [
            "Id",
            "Name",
            "InstalledVersion",
            "CatalogVersion",
            "Lifecycle",
            "ServiceState",
            "Status",
            "Error",
            "SettingsPage",
            "DashboardComponent",
        ] {
            self.add(&format!("{root}/{suffix}"), BusItem::string(""))?;
        }
        for suffix in [
            "Installed",
            "Available",
            "HasUpdate",
            "HasSettingsPage",
            "HasDashboard",
        ] {
            self.add(&format!("{root}/{suffix}"), BusItem::i32(0))?;
        }

        let id = plugin.id.clone();
        let sender = self.commands.clone();
        self.add(
            &format!("{root}/Enabled"),
            BusItem::writable_i32(0, move |value| {
                if value != 0 && value != 1 {
                    return 2;
                }
                sender
                    .send(ManagerCommand::SetEnabled(id.clone(), value == 1))
                    .map_or(2, |_| 0)
            }),
        )?;
        let id = plugin.id.clone();
        let sender = self.commands.clone();
        self.add(
            &format!("{root}/Install"),
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender
                    .send(ManagerCommand::Install(id.clone()))
                    .map_or(2, |_| 0)
            }),
        )?;
        let id = plugin.id.clone();
        let sender = self.commands.clone();
        self.add(
            &format!("{root}/Uninstall"),
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender
                    .send(ManagerCommand::Uninstall(id.clone()))
                    .map_or(2, |_| 0)
            }),
        )
    }

    fn publish_plugin(&self, plugin: &PluginSnapshot) -> zbus::Result<()> {
        let root = plugin_root(plugin);
        for (suffix, value) in [
            ("Id", plugin.id.as_str()),
            ("Name", plugin.name.as_str()),
            ("InstalledVersion", plugin.installed_version.as_str()),
            ("CatalogVersion", plugin.catalog_version.as_str()),
            ("Lifecycle", lifecycle_text(plugin.lifecycle)),
            ("ServiceState", service_text(plugin.service_state)),
            ("Status", plugin.status.as_str()),
            ("Error", plugin.error.as_str()),
            ("SettingsPage", plugin.settings_page.as_str()),
            ("DashboardComponent", plugin.dashboard_component.as_str()),
        ] {
            self.string(&format!("{root}/{suffix}"), value)?;
        }
        for (suffix, value) in [
            ("Installed", plugin.installed),
            ("Available", plugin.available),
            ("Enabled", plugin.enabled),
            ("HasUpdate", plugin.has_update),
            ("HasSettingsPage", !plugin.settings_page.is_empty()),
            ("HasDashboard", !plugin.dashboard_component.is_empty()),
        ] {
            self.i32(&format!("{root}/{suffix}"), i32::from(value))?;
        }
        self.i32(&format!("{root}/Install"), 0)?;
        self.i32(&format!("{root}/Uninstall"), 0)
    }

    fn add(&mut self, path: &str, item: BusItem) -> zbus::Result<()> {
        let handle = item.handle();
        self.connection.object_server().at(path, item)?;
        self.handles
            .lock()
            .expect("D-Bus handles poisoned")
            .insert(path.to_owned(), handle);
        Ok(())
    }

    fn string(&self, path: &str, value: &str) -> zbus::Result<()> {
        self.handles
            .lock()
            .expect("D-Bus handles poisoned")
            .get(path)
            .unwrap_or_else(|| panic!("missing D-Bus path {path}"))
            .set_string(&self.connection, path, value)
    }

    fn i32(&self, path: &str, value: i32) -> zbus::Result<()> {
        self.handles
            .lock()
            .expect("D-Bus handles poisoned")
            .get(path)
            .unwrap_or_else(|| panic!("missing D-Bus path {path}"))
            .set_i32(&self.connection, path, value)
    }
}

struct RootBusItem {
    handles: Arc<Mutex<HashMap<String, BusItemHandle>>>,
}

#[interface(name = "com.victronenergy.BusItem")]
impl RootBusItem {
    #[zbus(name = "GetItems")]
    fn get_items(&self) -> HashMap<String, HashMap<String, OwnedValue>> {
        self.handles
            .lock()
            .expect("D-Bus handles poisoned")
            .iter()
            .map(|(path, handle)| (path.clone(), handle.snapshot()))
            .collect()
    }
}

fn plugin_root(plugin: &PluginSnapshot) -> String {
    format!("/Plugins/{}", plugin.path_key)
}

fn plugin_key_from_item_path(path: &str) -> Option<&str> {
    path.strip_prefix("/Plugins/")?
        .split_once('/')
        .map(|(key, _)| key)
}

fn lifecycle_text(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Disabled => "disabled",
        LifecycleState::Enabled => "enabled",
        LifecycleState::Converging => "converging",
        LifecycleState::Degraded => "degraded",
    }
}

fn service_text(state: ServiceState) -> &'static str {
    match state {
        ServiceState::NotApplicable => "not-applicable",
        ServiceState::Stopped => "stopped",
        ServiceState::Running => "running",
        ServiceState::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::plugin_key_from_item_path;

    #[test]
    fn identifies_only_dynamic_plugin_item_paths() {
        assert_eq!(
            plugin_key_from_item_path("/Plugins/tpms/Installed"),
            Some("tpms")
        );
        assert_eq!(plugin_key_from_item_path("/InstalledCount"), None);
        assert_eq!(plugin_key_from_item_path("/Plugins/tpms"), None);
    }
}
