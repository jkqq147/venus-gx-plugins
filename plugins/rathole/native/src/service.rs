use std::{env, io, path::PathBuf, sync::mpsc};

use zbus::blocking::Connection;

use crate::{
    config::{LoadMode, ManagedConfig, ServiceConfig, MAX_SERVICES},
    process::RatholeProcess,
    publisher::{Command, Publisher},
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config_root = env::var_os("VENUS_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/venus-gx-plugins/config/rathole"));
    let config_path = config_root.join("client.toml");
    let binary_path = env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing executable directory"))?
        .join("rathole");

    let loaded = ManagedConfig::load(&config_path)?;
    let mut mode = loaded.mode;
    let mut draft = loaded.draft;
    let mut committed = if mode == LoadMode::Managed {
        draft.clone()
    } else {
        None
    };
    let mut dirty = false;
    let mut rename_pending = false;
    let mut add = AddEditor::default();

    let connection = Connection::system()?;
    let (sender, receiver) = mpsc::channel();
    let publisher = Publisher::new(connection, sender.clone())?;
    publisher.publish_mode(mode, loaded.detail)?;
    if let Some(config) = &draft {
        publisher.publish_config(config)?;
    }
    publisher.set_original_device_name(
        committed
            .as_ref()
            .map(|config| config.device_name.as_str())
            .unwrap_or(""),
    )?;
    publisher.publish_add_editor(&add.preset, &add.slug, &add.host, add.port)?;
    publisher.set_dirty(false)?;

    let mut child = None;
    let mut generation = 0_u64;
    let mut pending_restart = false;
    match mode {
        LoadMode::Managed | LoadMode::Advanced => start_child(
            &publisher,
            &sender,
            &binary_path,
            &config_path,
            &mut child,
            &mut generation,
        )?,
        LoadMode::Missing => publisher.set_status("not-configured", "Not configured")?,
        LoadMode::Invalid => publisher.set_status("invalid-config", "Invalid configuration")?,
    }

    // Register only after all paths and the initial snapshot are coherent.
    publisher.register()?;

    while let Ok(command) = receiver.recv() {
        match command {
            Command::ChildExited {
                generation: event_generation,
                success,
            } if event_generation == generation => {
                if let Some(process) = child.as_mut() {
                    process.mark_exited();
                }
                child = None;
                if pending_restart {
                    pending_restart = false;
                    start_child(
                        &publisher,
                        &sender,
                        &binary_path,
                        &config_path,
                        &mut child,
                        &mut generation,
                    )?;
                } else if success {
                    publisher.set_status("stopped", "Rathole stopped")?;
                } else {
                    publisher.set_status("failed", "Rathole exited unexpectedly")?;
                }
            }
            Command::ChildExited { .. } => {}
            command if !editable(mode) => {
                let _ = command;
                publisher.set_config_feedback("read-only", "Configuration is read-only")?;
            }
            Command::SetServerHost(value) => {
                if let Some(config) = draft.as_mut() {
                    config.server_host = value;
                    changed(&publisher, &mut dirty)?;
                    publisher.publish_config(config)?;
                }
            }
            Command::SetServerPort(value) => {
                if let (Some(config), Ok(port)) = (draft.as_mut(), value.trim().parse::<u16>()) {
                    if port != 0 {
                        config.server_port = port;
                        changed(&publisher, &mut dirty)?;
                        publisher.publish_config(config)?;
                    }
                }
            }
            Command::SetDeviceName(value) => {
                if let Some(config) = draft.as_mut() {
                    config.device_name = value;
                    changed(&publisher, &mut dirty)?;
                    publisher.publish_config(config)?;
                }
            }
            Command::SetServiceSlug(index, value) => {
                if let Some(config) = draft.as_mut() {
                    if let Some(service) = config.services.get_mut(index) {
                        service.slug = value;
                        changed(&publisher, &mut dirty)?;
                        publisher.publish_services(config)?;
                    }
                }
            }
            Command::SetServiceHost(index, value) => {
                if let Some(config) = draft.as_mut() {
                    if let Some(service) = config.services.get_mut(index) {
                        service.local_host = value;
                        changed(&publisher, &mut dirty)?;
                        publisher.publish_services(config)?;
                    }
                }
            }
            Command::SetServicePort(index, value) => {
                if let (Some(config), Ok(port)) = (draft.as_mut(), value.trim().parse::<u16>()) {
                    if let Some(service) = config.services.get_mut(index).filter(|_| port != 0) {
                        service.local_port = port;
                        changed(&publisher, &mut dirty)?;
                        publisher.publish_services(config)?;
                    }
                }
            }
            Command::DeleteService(index) => {
                if let Some(config) = draft.as_mut() {
                    if index < config.services.len() {
                        config.services.remove(index);
                        changed(&publisher, &mut dirty)?;
                        publisher.publish_services(config)?;
                    }
                }
            }
            Command::SetAddPreset(value) => {
                if let Some(preset) = Preset::from_name(&value) {
                    add.apply(preset);
                    publisher.publish_add_editor(&add.preset, &add.slug, &add.host, add.port)?;
                }
            }
            Command::SetAddSlug(value) => {
                add.slug = value;
                publisher.publish_add_editor(&add.preset, &add.slug, &add.host, add.port)?;
            }
            Command::SetAddHost(value) => {
                add.host = value;
                publisher.publish_add_editor(&add.preset, &add.slug, &add.host, add.port)?;
            }
            Command::SetAddPort(value) => {
                if let Ok(port) = value.trim().parse::<u16>() {
                    if port != 0 {
                        add.port = port;
                        publisher.publish_add_editor(
                            &add.preset,
                            &add.slug,
                            &add.host,
                            add.port,
                        )?;
                    }
                }
            }
            Command::AddService => {
                if let Some(config) = draft.as_mut() {
                    if config.services.len() >= MAX_SERVICES {
                        publisher.set_config_feedback(
                            "invalid-input",
                            "Maximum service count reached",
                        )?;
                    } else {
                        let mut candidate = config.clone();
                        candidate.services.push(ServiceConfig {
                            slug: add.slug.clone(),
                            local_host: add.host.clone(),
                            local_port: add.port,
                        });
                        candidate.normalize();
                        if service_fields_valid(&candidate) {
                            *config = candidate;
                            changed(&publisher, &mut dirty)?;
                            publisher.publish_config(config)?;
                            add = AddEditor::default();
                            publisher.publish_add_editor(
                                &add.preset,
                                &add.slug,
                                &add.host,
                                add.port,
                            )?;
                            publisher.set_config_feedback("unsaved", "Unsaved changes")?;
                        } else {
                            publisher.set_config_feedback("invalid-input", "Invalid service")?;
                        }
                    }
                }
            }
            Command::Save => {
                save(
                    false,
                    &publisher,
                    &sender,
                    &binary_path,
                    &config_path,
                    &mut mode,
                    &mut draft,
                    &mut committed,
                    &mut dirty,
                    &mut rename_pending,
                    &mut child,
                    &mut generation,
                    &mut pending_restart,
                )?;
            }
            Command::ConfirmRename => {
                save(
                    true,
                    &publisher,
                    &sender,
                    &binary_path,
                    &config_path,
                    &mut mode,
                    &mut draft,
                    &mut committed,
                    &mut dirty,
                    &mut rename_pending,
                    &mut child,
                    &mut generation,
                    &mut pending_restart,
                )?;
            }
        }
        publisher.reset_commands()?;
    }
    Ok(())
}

fn editable(mode: LoadMode) -> bool {
    matches!(mode, LoadMode::Missing | LoadMode::Managed)
}

fn changed(publisher: &Publisher, dirty: &mut bool) -> zbus::Result<()> {
    *dirty = true;
    publisher.set_dirty(true)?;
    publisher.set_rename_confirmation(false)?;
    publisher.set_config_feedback("unsaved", "Unsaved changes")
}

#[allow(clippy::too_many_arguments)]
fn save(
    confirmed_rename: bool,
    publisher: &Publisher,
    sender: &mpsc::Sender<Command>,
    binary_path: &std::path::Path,
    config_path: &std::path::Path,
    mode: &mut LoadMode,
    draft: &mut Option<ManagedConfig>,
    committed: &mut Option<ManagedConfig>,
    dirty: &mut bool,
    rename_pending: &mut bool,
    child: &mut Option<RatholeProcess>,
    generation: &mut u64,
    pending_restart: &mut bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !*dirty {
        publisher.set_config_feedback("unchanged", "No changes")?;
        return Ok(());
    }
    let Some(current) = draft.as_ref() else {
        return Ok(());
    };
    let mut candidate = current.clone();
    candidate.normalize();
    if let Err(error) = candidate.validate() {
        publisher.set_config_feedback("invalid-input", &error.to_string())?;
        return Ok(());
    }

    let renamed = rename_requires_confirmation(committed.as_ref(), &candidate);
    if renamed && !confirmed_rename {
        *rename_pending = true;
        publisher.set_rename_confirmation(true)?;
        publisher.set_config_feedback(
            "rename-confirmation",
            "Changing the device name requires matching server changes",
        )?;
        return Ok(());
    }

    if committed.as_ref() == Some(&candidate) {
        *draft = Some(candidate.clone());
        *dirty = false;
        *rename_pending = false;
        publisher.publish_config(&candidate)?;
        publisher.set_original_device_name(&candidate.device_name)?;
        publisher.set_dirty(false)?;
        publisher.set_rename_confirmation(false)?;
        publisher.set_config_feedback("unchanged", "No changes")?;
        return Ok(());
    }

    let written = candidate.save_if_changed(config_path)?;
    *mode = LoadMode::Managed;
    *committed = Some(candidate.clone());
    *draft = Some(candidate.clone());
    *dirty = false;
    *rename_pending = false;
    publisher.publish_mode(LoadMode::Managed, "ready")?;
    publisher.publish_config(&candidate)?;
    publisher.set_original_device_name(&candidate.device_name)?;
    publisher.set_dirty(false)?;
    publisher.set_rename_confirmation(false)?;
    publisher.set_config_feedback("saved", "Configuration saved")?;

    if written {
        if let Some(process) = child.as_mut() {
            publisher.set_status("restarting", "Restarting Rathole")?;
            process.stop()?;
            *pending_restart = true;
        } else {
            start_child(
                publisher,
                sender,
                binary_path,
                config_path,
                child,
                generation,
            )?;
        }
    } else {
        publisher.set_config_feedback("unchanged", "No changes")?;
    }
    Ok(())
}

fn start_child(
    publisher: &Publisher,
    sender: &mpsc::Sender<Command>,
    binary_path: &std::path::Path,
    config_path: &std::path::Path,
    child: &mut Option<RatholeProcess>,
    generation: &mut u64,
) -> Result<(), Box<dyn std::error::Error>> {
    *generation = generation.wrapping_add(1);
    publisher.set_status("starting", "Starting Rathole")?;
    match RatholeProcess::start(binary_path, config_path, *generation, sender.clone()) {
        Ok(process) => {
            *child = Some(process);
            publisher.set_status("running", "Running")?;
        }
        Err(error) => {
            publisher.set_status("failed", &format!("Cannot start Rathole: {error}"))?;
        }
    }
    Ok(())
}

fn service_fields_valid(config: &ManagedConfig) -> bool {
    if config.services.is_empty() {
        return false;
    }
    let probe = ManagedConfig {
        server_host: "127.0.0.1".to_owned(),
        server_port: 2333,
        device_name: "probe".to_owned(),
        token: "PROBE-TOKEN".to_owned(),
        services: config.services.clone(),
    };
    probe.validate().is_ok()
}

fn rename_requires_confirmation(
    committed: Option<&ManagedConfig>,
    candidate: &ManagedConfig,
) -> bool {
    committed.is_some_and(|saved| {
        saved.device_name != candidate.device_name && !saved.services.is_empty()
    })
}

struct AddEditor {
    preset: String,
    slug: String,
    host: String,
    port: u16,
}

impl Default for AddEditor {
    fn default() -> Self {
        Self {
            preset: "homeassistant".to_owned(),
            slug: "homeassistant".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 8123,
        }
    }
}

impl AddEditor {
    fn apply(&mut self, preset: Preset) {
        self.preset = preset.name().to_owned();
        self.slug = preset.slug().to_owned();
        self.port = preset.port();
    }
}

#[derive(Clone, Copy)]
enum Preset {
    HomeAssistant,
    Loxone,
    Hikvision,
    Frigate,
    Ssh,
    Custom,
}

impl Preset {
    fn from_name(value: &str) -> Option<Self> {
        Some(match value {
            "homeassistant" => Self::HomeAssistant,
            "loxone" => Self::Loxone,
            "hikvision" => Self::Hikvision,
            "frigate" => Self::Frigate,
            "ssh" => Self::Ssh,
            "custom" => Self::Custom,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::HomeAssistant => "homeassistant",
            Self::Loxone => "loxone",
            Self::Hikvision => "hikvision",
            Self::Frigate => "frigate",
            Self::Ssh => "ssh",
            Self::Custom => "custom",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Custom => "service",
            _ => self.name(),
        }
    }

    fn port(self) -> u16 {
        match self {
            Self::HomeAssistant => 8123,
            Self::Loxone => 80,
            Self::Hikvision => 8000,
            Self::Frigate => 8971,
            Self::Ssh => 22,
            Self::Custom => 80,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(device_name: &str) -> ManagedConfig {
        ManagedConfig {
            server_host: "106.55.191.108".into(),
            server_port: 2333,
            device_name: device_name.into(),
            token: "7K4M-2D9Q".into(),
            services: vec![ServiceConfig {
                slug: "loxone".into(),
                local_host: "192.168.50.2".into(),
                local_port: 80,
            }],
        }
    }

    #[test]
    fn changing_a_live_device_name_requires_confirmation() {
        let committed = config("sn1350");
        let candidate = config("sn1351");
        assert!(rename_requires_confirmation(Some(&committed), &candidate));
        assert!(!rename_requires_confirmation(Some(&candidate), &candidate));
        assert!(!rename_requires_confirmation(None, &candidate));
    }

    #[test]
    fn add_editor_rejects_a_duplicate_generated_service_name() {
        let mut candidate = config("sn1350");
        candidate.services.push(candidate.services[0].clone());
        assert!(!service_fields_valid(&candidate));
    }
}
