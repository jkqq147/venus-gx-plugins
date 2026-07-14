use std::{env, path::PathBuf, sync::mpsc, time::Duration};

use zbus::blocking::Connection;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    config::{Config, Credentials},
    discovery::{self, DiscoveredMiniserver},
    loxone::{self, Session},
    probe::{require_unique_tanks, TankKind, TankSensorCandidate},
    publisher::{Command, Publisher, DISCOVERY_LIMIT},
    runtime::RuntimeHandle,
    tank::TankServices,
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config_root = env::var_os("VENUS_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/venus-gx-plugins/config/loxone-tanks"));
    let config_path = config_root.join("settings.json");
    let credentials_path = config_root.join("credentials.json");
    let config = Config::load(&config_path)?;
    let mut credentials = Credentials::load(&credentials_path)?;
    let mut committed = config.clone();
    let mut draft = Draft {
        config,
        password: Zeroizing::new(String::new()),
    };
    let mut results: Vec<DiscoveredMiniserver> = Vec::new();

    let connection = Connection::system()?;
    let (sender, receiver) = mpsc::channel();
    let mut tanks = TankServices::new(&committed, sender.clone())?;
    let publisher = Publisher::new(connection, sender.clone())?;
    publisher.publish_config(&draft.config)?;

    let mut generation = 0_u64;
    let mut runtime = None;
    if is_configured(&committed) {
        if let Some(saved_credentials) = credentials.clone() {
            start_runtime(
                &publisher,
                &sender,
                &mut runtime,
                &mut generation,
                committed.clone(),
                saved_credentials,
            )?;
        } else {
            publisher.set_connection("credentials-required", "Credentials required")?;
        }
    } else {
        publisher.set_connection("not-configured", "Not configured")?;
    }

    // Register only after all static paths and the initial state are ready so
    // the GUI's first root GetItems call observes a coherent snapshot.
    tanks.register()?;
    publisher.register()?;

    while let Ok(command) = receiver.recv() {
        match command {
            Command::ScanMiniservers => {
                publisher.set_discovery_state("scanning")?;
                results = discovery::discover(Duration::from_millis(1_500));
                results.truncate(DISCOVERY_LIMIT);
                publisher.publish_discovery(&results)?;
                publisher.set_discovery_state(if results.is_empty() {
                    "not-found"
                } else {
                    "complete"
                })?;
            }
            Command::SelectMiniserver(index) => {
                if let Some(server) = results.get(index) {
                    draft.config.miniserver.host = server.address.clone();
                    publisher.set_host(&draft.config.miniserver.host)?;
                    publisher.set_connection("selected", "Server selected")?;
                }
            }
            Command::SetHost(value) => match discovery::normalize_address(&value) {
                Ok(host) => {
                    draft.config.miniserver.host = host.clone();
                    publisher.set_host(&host)?;
                }
                Err(_) => publisher.set_connection("invalid-address", "Invalid address")?,
            },
            Command::SetUsername(value) => {
                let username = value.trim();
                if username.len() <= 64 && !username.bytes().any(|byte| byte.is_ascii_control()) {
                    draft.config.miniserver.username = username.to_owned();
                    publisher.set_username(username)?;
                } else {
                    publisher.set_connection("invalid-username", "Invalid username")?;
                }
            }
            Command::SetPassword(value) => {
                draft.password.zeroize();
                *draft.password = value;
            }
            Command::SetTankCapacity(tank, capacity_liters) => {
                save_tank_capacity(&mut committed, tank, capacity_liters, &config_path)?;
                binding_mut(&mut draft.config, tank).capacity_liters = capacity_liters;
                tanks.set_capacity(tank, capacity_liters)?;
            }
            Command::SaveServer => {
                save_and_connect(
                    &publisher,
                    &sender,
                    &config_path,
                    &credentials_path,
                    &mut draft,
                    &mut committed,
                    &mut credentials,
                    &mut tanks,
                    &mut runtime,
                    &mut generation,
                )?;
            }
            Command::Reconnect => {
                if let Some(saved_credentials) =
                    credentials.clone().filter(|_| is_configured(&committed))
                {
                    start_runtime(
                        &publisher,
                        &sender,
                        &mut runtime,
                        &mut generation,
                        committed.clone(),
                        saved_credentials,
                    )?;
                } else {
                    publisher
                        .set_connection("credentials-required", "Verify the configuration first")?;
                }
            }
            Command::RuntimeConnected(event_generation) if event_generation == generation => {
                publisher.set_runtime_connected()?;
            }
            Command::RuntimeValues(event_generation, values) if event_generation == generation => {
                for (state_uuid, value) in values {
                    if let Some(tank) = tank_for_state_uuid(&committed, &state_uuid) {
                        tanks.set_level(tank, value)?;
                    }
                }
            }
            Command::RuntimeCredentials(event_generation, refreshed)
                if event_generation == generation =>
            {
                refreshed.save_if_changed(&credentials_path)?;
                credentials = Some(refreshed);
            }
            Command::RuntimeDisconnected(event_generation, message)
                if event_generation == generation =>
            {
                publisher.set_runtime_disconnected(&message)?;
                tanks.set_disconnected()?;
                runtime = None;
            }
            Command::RuntimeConnected(_)
            | Command::RuntimeValues(_, _)
            | Command::RuntimeCredentials(_, _)
            | Command::RuntimeDisconnected(_, _) => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn save_and_connect(
    publisher: &Publisher,
    sender: &mpsc::Sender<Command>,
    config_path: &std::path::Path,
    credentials_path: &std::path::Path,
    draft: &mut Draft,
    committed: &mut Config,
    credentials: &mut Option<Credentials>,
    tanks: &mut TankServices,
    runtime: &mut Option<RuntimeHandle>,
    generation: &mut u64,
) -> Result<(), Box<dyn std::error::Error>> {
    if draft.config.miniserver.username.is_empty() {
        publisher.set_connection("invalid-username", "Username is required")?;
        return Ok(());
    }
    let host = draft.config.miniserver.host.clone();
    if let Err(error) = discovery::verify_miniserver(&host, Duration::from_secs(3)) {
        publisher.set_connection("connection-failed", &format!("Connection failed: {error}"))?;
        draft.password.zeroize();
        return Ok(());
    }

    let server_identity_changed = draft.config.miniserver != committed.miniserver;
    let provisioned = if draft.password.is_empty() {
        if server_identity_changed || credentials.is_none() {
            publisher.set_connection("credentials-required", "Password is required")?;
            return Ok(());
        }
        None
    } else {
        let identity = credentials
            .as_ref()
            .map(|value| value.client_uuid.clone())
            .unwrap_or_else(|| {
                let machine = loxone::machine_identity("venus-gx");
                loxone::client_uuid(&machine)
            });
        publisher.set_connection("authenticating", "Authenticating")?;
        let result = Session::provision(
            &draft.config.miniserver.host,
            &draft.config.miniserver.username,
            &draft.password,
            &identity,
        );
        draft.password.zeroize();
        match result {
            Ok(provisioned) => Some(provisioned),
            Err(error) => {
                let state = if matches!(error, crate::loxone::LoxoneError::Authentication) {
                    "authentication-failed"
                } else {
                    "connection-failed"
                };
                publisher.set_connection(state, &public_error(&error))?;
                return Ok(());
            }
        }
    };

    if let Some(provisioned) = provisioned {
        let candidates = match require_unique_tanks(&provisioned.candidates) {
            Ok(candidates) => candidates,
            Err(error) => {
                publisher.set_connection("sensor-error", &error.to_string())?;
                return Ok(());
            }
        };
        apply_bindings(&mut draft.config, &candidates);
        provisioned.credentials.save_if_changed(credentials_path)?;
        *credentials = Some(provisioned.credentials);
    }

    draft.config.save_if_changed(config_path)?;
    *committed = draft.config.clone();
    publisher.publish_config(committed)?;
    for tank in TankKind::ALL {
        tanks.set_capacity(tank, binding(committed, tank).capacity_liters)?;
    }

    if let Some(saved_credentials) = credentials.clone() {
        start_runtime(
            publisher,
            sender,
            runtime,
            generation,
            committed.clone(),
            saved_credentials,
        )?;
    }
    Ok(())
}

fn start_runtime(
    publisher: &Publisher,
    sender: &mpsc::Sender<Command>,
    runtime: &mut Option<RuntimeHandle>,
    generation: &mut u64,
    config: Config,
    credentials: Credentials,
) -> zbus::Result<()> {
    if let Some(worker) = runtime.as_mut() {
        worker.stop();
    }
    *generation = generation.wrapping_add(1);
    publisher.set_connection("connecting", "Connecting")?;
    *runtime = Some(RuntimeHandle::start(
        *generation,
        config,
        credentials,
        sender.clone(),
    ));
    Ok(())
}

fn is_configured(config: &Config) -> bool {
    !config.miniserver.host.is_empty()
        && !config.miniserver.username.is_empty()
        && TankKind::ALL
            .into_iter()
            .all(|tank| !binding(config, tank).state_uuid.is_empty())
}

fn apply_bindings(config: &mut Config, candidates: &[TankSensorCandidate]) {
    for candidate in candidates {
        binding_mut(config, candidate.tank).state_uuid = candidate.state_uuid.clone();
    }
}

fn tank_for_state_uuid(config: &Config, state_uuid: &str) -> Option<TankKind> {
    TankKind::ALL.into_iter().find(|tank| {
        binding(config, *tank)
            .state_uuid
            .eq_ignore_ascii_case(state_uuid)
    })
}

fn binding(config: &Config, tank: TankKind) -> &crate::config::TankBinding {
    match tank {
        TankKind::Fresh => &config.tanks.fresh,
        TankKind::Gray => &config.tanks.gray,
        TankKind::Black => &config.tanks.black,
    }
}

fn binding_mut(config: &mut Config, tank: TankKind) -> &mut crate::config::TankBinding {
    match tank {
        TankKind::Fresh => &mut config.tanks.fresh,
        TankKind::Gray => &mut config.tanks.gray,
        TankKind::Black => &mut config.tanks.black,
    }
}

fn save_tank_capacity(
    config: &mut Config,
    tank: TankKind,
    capacity_liters: f64,
    path: &std::path::Path,
) -> std::io::Result<bool> {
    let mut updated = config.clone();
    binding_mut(&mut updated, tank).capacity_liters = capacity_liters;
    let changed = updated.save_if_changed(path)?;
    *config = updated;
    Ok(changed)
}

fn public_error(error: &crate::loxone::LoxoneError) -> String {
    match error {
        crate::loxone::LoxoneError::Authentication => "Authentication failed".to_owned(),
        crate::loxone::LoxoneError::Timeout => "Miniserver did not respond".to_owned(),
        _ => error.to_string(),
    }
}

struct Draft {
    config: Config,
    password: Zeroizing<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capacity_change_is_persisted_only_when_value_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut config = Config::default();

        assert!(save_tank_capacity(&mut config, TankKind::Fresh, 180.0, &path).unwrap());
        assert_eq!(
            Config::load(&path).unwrap().tanks.fresh.capacity_liters,
            180.0
        );
        assert!(!save_tank_capacity(&mut config, TankKind::Fresh, 180.0, &path).unwrap());
    }
}
