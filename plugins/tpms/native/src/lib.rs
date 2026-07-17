//! Native BLE scanning, state management, and Venus D-Bus publishing.

#[cfg(any(target_os = "linux", test))]
use std::time::Duration;

#[cfg(any(target_os = "linux", test))]
const ADAPTER_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(any(target_os = "linux", test))]
fn adapter_retry_due(discovery_active: bool, elapsed: Duration) -> bool {
    !discovery_active && elapsed >= ADAPTER_RETRY_INTERVAL
}

#[cfg(target_os = "linux")]
mod bluez;
#[cfg(target_os = "linux")]
mod bus_item;
#[cfg(target_os = "linux")]
mod event;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
mod state;
#[cfg(target_os = "linux")]
mod venus;

#[cfg(target_os = "linux")]
pub fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, io, sync::mpsc, time::Instant};

    use event::Event;
    use state::{unix_time, TpmsState};
    use venus::{BluetoothStats, VenusPublisher};
    use zbus::blocking::Connection;

    const DEFAULT_SERVICE_NAME: &str = "com.victronenergy.tpms.main";
    const DEFAULT_STATE_PATH: &str = "/data/venus-gx-plugins/config/tpms/state.json";
    const STALE_SECONDS: i64 = 300;
    const DISCOVERED_MAX_AGE_SECONDS: i64 = 300;

    let service_name =
        env::var("VENUS_TPMS_SERVICE_NAME").unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_owned());
    let state_path =
        env::var("VENUS_TPMS_STATE_PATH").unwrap_or_else(|_| DEFAULT_STATE_PATH.to_owned());
    let connection = Connection::system()?;
    let (events_tx, events_rx) = mpsc::channel();
    let publisher = VenusPublisher::new(connection.clone(), &service_name, events_tx.clone())?;
    let mut state = TpmsState::load(state_path)?;
    let mut bluetooth_stats = BluetoothStats::default();
    publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;

    eprintln!("publishing {service_name} from native HCI");
    let mut last_maintenance = Instant::now();
    let mut last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
    let mut last_discovery_check = Instant::now() - Duration::from_secs(5);
    let mut discovery = None;
    let mut scanner_running = false;
    loop {
        if adapter_retry_due(discovery.is_some(), last_adapter_attempt.elapsed()) {
            last_adapter_attempt = Instant::now();
            if let Ok(adapter_index) = bluez::lowest_adapter_index() {
                if let Ok(active_discovery) =
                    bluez::BluezDiscovery::start(&connection, adapter_index)
                {
                    discovery = Some(active_discovery);
                    bluetooth_stats = BluetoothStats {
                        adapter_index: Some(adapter_index),
                        active: true,
                        ..BluetoothStats::default()
                    };
                    if !scanner_running {
                        spawn_scanner(adapter_index, events_tx.clone());
                        scanner_running = true;
                    }
                    publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
                }
            }
        }

        if last_discovery_check.elapsed() >= Duration::from_secs(5) {
            if let Some(active_discovery) = discovery.as_ref() {
                if !active_discovery.is_active() {
                    eprintln!("BlueZ discovery stopped; reconnecting");
                    discovery = None;
                    last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                    bluetooth_stats = BluetoothStats::default();
                    publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
                }
            }
            last_discovery_check = Instant::now();
        }

        match events_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Reading(reading)) => {
                state.update(reading);
                publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
            }
            Ok(Event::ScanStats {
                device_count,
                manufacturer_data_count,
            }) => {
                bluetooth_stats.device_count = device_count;
                bluetooth_stats.manufacturer_data_count = manufacturer_data_count;
                publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
            }
            Ok(Event::Assign { index, wheel }) => {
                let sensor_id = state
                    .discovered()
                    .get(index)
                    .map(|reading| reading.sensor_id.clone());
                if let Some(sensor_id) = sensor_id {
                    state.bind(wheel, &sensor_id)?;
                    publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
                }
            }
            Ok(Event::ScannerFailed(error)) => {
                eprintln!("HCI scanner stopped: {error}");
                discovery = None;
                last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                scanner_running = false;
                bluetooth_stats = BluetoothStats::default();
                publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("HCI event channel disconnected").into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_maintenance.elapsed() >= Duration::from_secs(30) {
            state.prune_expired_unbound(unix_time(), DISCOVERED_MAX_AGE_SECONDS);
            publisher.publish(&state, STALE_SECONDS, bluetooth_stats)?;
            last_maintenance = Instant::now();
        }
    }
}

#[cfg(target_os = "linux")]
pub fn run_hci_monitor() -> std::io::Result<()> {
    linux::run()
}

#[cfg(target_os = "linux")]
fn spawn_scanner(adapter_index: u16, events_tx: std::sync::mpsc::Sender<event::Event>) {
    use std::thread;

    use event::Event;
    use state::{unix_time, Reading};

    thread::spawn(move || {
        let scanner_tx = events_tx.clone();
        if let Err(error) = linux::run_with_handlers(
            adapter_index,
            move |observation| {
                let parsed = observation.reading;
                let reading = Reading {
                    sensor_id: parsed.sensor_id_hex,
                    name: observation.local_name.unwrap_or_default(),
                    pressure_bar: parsed.pressure_bar as f64,
                    temperature_c: parsed.temperature_c as f64,
                    battery_percent: parsed.battery_percent as i32,
                    alarm: parsed.alarm as i32,
                    rssi: observation.rssi as i32,
                    last_seen: unix_time(),
                    manufacturer_data_hex: observation.manufacturer_data_hex,
                };
                let _ = scanner_tx.send(Event::Reading(reading));
            },
            {
                let stats_tx = events_tx.clone();
                move |stats| {
                    let _ = stats_tx.send(Event::ScanStats {
                        device_count: stats.device_count,
                        manufacturer_data_count: stats.manufacturer_data_count,
                    });
                }
            },
        ) {
            let _ = events_tx.send(Event::ScannerFailed(error.to_string()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{adapter_retry_due, ADAPTER_RETRY_INTERVAL};
    use std::time::Duration;

    #[test]
    fn missing_adapter_is_retried_only_after_the_cooldown() {
        assert!(!adapter_retry_due(false, Duration::from_secs(59)));
        assert!(adapter_retry_due(false, ADAPTER_RETRY_INTERVAL));
    }

    #[test]
    fn active_discovery_never_schedules_an_adapter_retry() {
        assert!(!adapter_retry_due(true, Duration::MAX));
    }
}
