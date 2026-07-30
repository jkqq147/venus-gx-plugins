//! Native BLE and 433 MHz scanning, state management, and Venus D-Bus publishing.

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
mod source;
#[cfg(any(target_os = "linux", test))]
mod state;
#[cfg(target_os = "linux")]
mod venus;

#[cfg(target_os = "linux")]
pub fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, io, path::PathBuf, sync::mpsc, time::Instant};

    use event::Event;
    use source::{
        ble,
        rtl433::{retry_delay_seconds, Rtl433Process},
    };
    use state::{unix_time, TpmsState};
    use venus::{ReceiverStats, VenusPublisher};
    use zbus::blocking::Connection;

    const DEFAULT_SERVICE_NAME: &str = "com.victronenergy.tpms.main";
    const DEFAULT_STATE_PATH: &str = "/data/venus-gx-plugins/config/tpms/state.json";

    let service_name =
        env::var("VENUS_TPMS_SERVICE_NAME").unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_owned());
    let state_path =
        env::var("VENUS_TPMS_STATE_PATH").unwrap_or_else(|_| DEFAULT_STATE_PATH.to_owned());
    let connection = Connection::system()?;
    let (events_tx, events_rx) = mpsc::channel();
    let publisher = VenusPublisher::new(connection.clone(), &service_name, events_tx.clone())?;
    let mut state = TpmsState::load(state_path)?;
    let mut receivers = ReceiverStats::default();
    publisher.publish(&state, &receivers)?;

    let rtl433_binary = env::var_os("VENUS_TPMS_RTL433_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rtl_433")
        });
    eprintln!("publishing {service_name} from BLE and rtl_433");
    let mut last_maintenance = Instant::now();
    let mut last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
    let mut last_discovery_check = Instant::now() - Duration::from_secs(5);
    let mut discovery = None;
    let mut scanner_running = false;
    let mut rtl433_process = None;
    let mut rtl433_generation = 0_u64;
    let mut rtl433_failures = 0_u32;
    let mut rtl433_retry_seconds = 0_u64;
    let mut last_rtl433_attempt = Instant::now() - Duration::from_secs(60);
    loop {
        if rtl433_process.is_none()
            && last_rtl433_attempt.elapsed() >= Duration::from_secs(rtl433_retry_seconds)
        {
            last_rtl433_attempt = Instant::now();
            rtl433_generation = rtl433_generation.wrapping_add(1);
            match Rtl433Process::start(&rtl433_binary, rtl433_generation, events_tx.clone()) {
                Ok(process) => {
                    rtl433_process = Some(process);
                    rtl433_failures = 0;
                    rtl433_retry_seconds = 0;
                    receivers.rtl433.active = true;
                    receivers.rtl433.last_error.clear();
                    publisher.publish(&state, &receivers)?;
                }
                Err(error) => {
                    rtl433_failures = rtl433_failures.saturating_add(1);
                    rtl433_retry_seconds = retry_delay_seconds(rtl433_failures);
                    receivers.rtl433.active = false;
                    receivers.rtl433.last_error = error.to_string();
                    publisher.publish(&state, &receivers)?;
                }
            }
        }

        if adapter_retry_due(discovery.is_some(), last_adapter_attempt.elapsed()) {
            last_adapter_attempt = Instant::now();
            if let Ok(adapter_index) = bluez::lowest_adapter_index() {
                if let Ok(active_discovery) =
                    bluez::BluezDiscovery::start(&connection, adapter_index)
                {
                    discovery = Some(active_discovery);
                    receivers.bluetooth = venus::BluetoothStats {
                        adapter_index: Some(adapter_index),
                        active: true,
                        ..venus::BluetoothStats::default()
                    };
                    if !scanner_running {
                        ble::spawn(adapter_index, events_tx.clone());
                        scanner_running = true;
                    }
                    publisher.publish(&state, &receivers)?;
                }
            }
        }

        if last_discovery_check.elapsed() >= Duration::from_secs(5) {
            if let Some(active_discovery) = discovery.as_ref() {
                if !active_discovery.is_active() {
                    eprintln!("BlueZ discovery stopped; reconnecting");
                    discovery = None;
                    last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                    receivers.bluetooth = venus::BluetoothStats::default();
                    publisher.publish(&state, &receivers)?;
                }
            }
            last_discovery_check = Instant::now();
        }

        match events_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Reading(reading)) => {
                state.update(*reading);
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::BluetoothStats {
                device_count,
                manufacturer_data_count,
            }) => {
                receivers.bluetooth.device_count = device_count;
                receivers.bluetooth.manufacturer_data_count = manufacturer_data_count;
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Frame { generation }) if generation == rtl433_generation => {
                receivers.rtl433.frame_count = receivers.rtl433.frame_count.saturating_add(1);
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Exited {
                generation,
                success,
            }) if generation == rtl433_generation => {
                if let Some(process) = rtl433_process.as_mut() {
                    process.mark_exited();
                }
                rtl433_process = None;
                receivers.rtl433.active = false;
                rtl433_failures = rtl433_failures.saturating_add(1);
                rtl433_retry_seconds = retry_delay_seconds(rtl433_failures);
                receivers.rtl433.last_error = if success {
                    "rtl_433 stopped".to_owned()
                } else {
                    "rtl_433 exited unexpectedly".to_owned()
                };
                last_rtl433_attempt = Instant::now();
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Frame { .. }) | Ok(Event::Rtl433Exited { .. }) => {}
            Ok(Event::Assign { index, wheel }) => {
                let sensor_key = state
                    .discovered()
                    .get(index)
                    .map(|reading| reading.sensor_key.clone());
                if let Some(sensor_key) = sensor_key {
                    state.bind(wheel, &sensor_key)?;
                    publisher.publish(&state, &receivers)?;
                }
            }
            Ok(Event::ScannerFailed(error)) => {
                eprintln!("HCI scanner stopped: {error}");
                discovery = None;
                last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                scanner_running = false;
                receivers.bluetooth = venus::BluetoothStats::default();
                publisher.publish(&state, &receivers)?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("HCI event channel disconnected").into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_maintenance.elapsed() >= Duration::from_secs(30) {
            state.prune_expired_unbound(unix_time());
            publisher.publish(&state, &receivers)?;
            last_maintenance = Instant::now();
        }
    }
}

#[cfg(target_os = "linux")]
pub fn run_hci_monitor() -> std::io::Result<()> {
    linux::run()
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
