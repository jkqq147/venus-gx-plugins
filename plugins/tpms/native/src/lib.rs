//! Native BLE and 433 MHz scanning, state management, and Venus D-Bus publishing.

#[cfg(any(target_os = "linux", test))]
use std::time::Duration;

#[cfg(any(target_os = "linux", test))]
const ADAPTER_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(any(target_os = "linux", test))]
const RTL433_HEARTBEAT_TIMEOUT: Duration =
    Duration::from_secs(source::rtl433::STATS_HEARTBEAT_TIMEOUT_SECONDS);

#[cfg(any(target_os = "linux", test))]
fn adapter_retry_due(discovery_active: bool, elapsed: Duration) -> bool {
    !discovery_active && elapsed >= ADAPTER_RETRY_INTERVAL
}

#[cfg(any(target_os = "linux", test))]
fn rtl433_heartbeat_expired(process_active: bool, elapsed: Duration) -> bool {
    process_active && elapsed >= RTL433_HEARTBEAT_TIMEOUT
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
        ble::BleScanner,
        rtl433::{retry_delay_seconds, Rtl433Process},
    };
    use state::{unix_time, TpmsState};
    use venus::{BluetoothStats, ReceiverStats, VenusPublisher};
    use zbus::blocking::Connection;

    const DEFAULT_SERVICE_NAME: &str = "com.victronenergy.tpms.main";
    const DEFAULT_STATE_PATH: &str = "/data/venus-gx-plugins/config/tpms/state.json";

    let service_name =
        env::var("VENUS_TPMS_SERVICE_NAME").unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_owned());
    let state_path =
        env::var("VENUS_TPMS_STATE_PATH").unwrap_or_else(|_| DEFAULT_STATE_PATH.to_owned());
    let connection = Connection::system()?;
    let (events_tx, events_rx) = mpsc::channel();
    let mut state = TpmsState::load(state_path)?;
    let publisher = VenusPublisher::new(
        connection.clone(),
        &service_name,
        events_tx.clone(),
        state.receiver_mode(),
    )?;
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
    let mut ble_scanner: Option<BleScanner> = None;
    let mut ble_generation = 0_u64;
    let mut rtl433_process: Option<Rtl433Process> = None;
    let mut rtl433_generation = 0_u64;
    let mut rtl433_failures = 0_u32;
    let mut rtl433_retry_seconds = 0_u64;
    let mut last_rtl433_attempt = Instant::now() - Duration::from_secs(60);
    let mut last_rtl433_heartbeat = Instant::now();
    let mut applied_receiver_mode = None;
    loop {
        let receiver_mode = state.receiver_mode();
        if applied_receiver_mode != Some(receiver_mode) {
            if receiver_mode.uses_bluetooth() {
                if !receivers.bluetooth.enabled {
                    receivers.bluetooth = BluetoothStats {
                        enabled: true,
                        ..BluetoothStats::default()
                    };
                    last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                }
            } else {
                ble_generation = ble_generation.wrapping_add(1);
                discovery = None;
                if let Some(mut scanner) = ble_scanner.take() {
                    scanner.stop();
                }
                receivers.bluetooth = BluetoothStats::default();
            }

            if receiver_mode.uses_rtl433() {
                if !receivers.rtl433.enabled {
                    receivers.rtl433.enabled = true;
                    receivers.rtl433.active = false;
                    receivers.rtl433.ready = false;
                    receivers.rtl433.recent_fsk_activity = false;
                    receivers.rtl433.recent_valid_activity = false;
                    receivers.rtl433.last_error.clear();
                    rtl433_failures = 0;
                    rtl433_retry_seconds = 0;
                    last_rtl433_attempt = Instant::now() - Duration::from_secs(60);
                }
            } else {
                rtl433_generation = rtl433_generation.wrapping_add(1);
                let stop_error = if let Some(mut process) = rtl433_process.take() {
                    process.stop().err()
                } else {
                    None
                };
                receivers.rtl433.enabled = false;
                receivers.rtl433.active = false;
                receivers.rtl433.ready = false;
                receivers.rtl433.recent_fsk_activity = false;
                receivers.rtl433.recent_valid_activity = false;
                receivers.rtl433.last_error = stop_error
                    .map(|error| format!("failed to stop rtl_433: {error}"))
                    .unwrap_or_default();
                rtl433_failures = 0;
                rtl433_retry_seconds = 0;
            }
            applied_receiver_mode = Some(receiver_mode);
            publisher.publish(&state, &receivers)?;
        }

        if receivers.rtl433.enabled
            && rtl433_process.is_none()
            && last_rtl433_attempt.elapsed() >= Duration::from_secs(rtl433_retry_seconds)
        {
            last_rtl433_attempt = Instant::now();
            rtl433_generation = rtl433_generation.wrapping_add(1);
            match Rtl433Process::start(&rtl433_binary, rtl433_generation, events_tx.clone()) {
                Ok(process) => {
                    rtl433_process = Some(process);
                    last_rtl433_heartbeat = Instant::now();
                    receivers.rtl433.active = true;
                    receivers.rtl433.ready = false;
                    receivers.rtl433.recent_fsk_activity = false;
                    receivers.rtl433.recent_valid_activity = false;
                    publisher.publish(&state, &receivers)?;
                }
                Err(error) => {
                    rtl433_generation = rtl433_generation.wrapping_add(1);
                    rtl433_failures = rtl433_failures.saturating_add(1);
                    rtl433_retry_seconds = retry_delay_seconds(rtl433_failures);
                    receivers.rtl433.active = false;
                    receivers.rtl433.ready = false;
                    receivers.rtl433.recent_fsk_activity = false;
                    receivers.rtl433.recent_valid_activity = false;
                    receivers.rtl433.last_error = error.to_string();
                    publisher.publish(&state, &receivers)?;
                }
            }
        }

        if receivers.bluetooth.enabled
            && adapter_retry_due(discovery.is_some(), last_adapter_attempt.elapsed())
        {
            last_adapter_attempt = Instant::now();
            if let Ok(adapter_index) = bluez::lowest_adapter_index() {
                if let Ok(active_discovery) =
                    bluez::BluezDiscovery::start(&connection, adapter_index)
                {
                    ble_generation = ble_generation.wrapping_add(1);
                    match BleScanner::start(adapter_index, ble_generation, events_tx.clone()) {
                        Ok(scanner) => {
                            discovery = Some(active_discovery);
                            ble_scanner = Some(scanner);
                            receivers.bluetooth = BluetoothStats {
                                enabled: true,
                                adapter_index: Some(adapter_index),
                                active: true,
                                ..BluetoothStats::default()
                            };
                            publisher.publish(&state, &receivers)?;
                        }
                        Err(error) => {
                            drop(active_discovery);
                            eprintln!("failed to start HCI scanner: {error}");
                        }
                    }
                }
            }
        }

        if last_discovery_check.elapsed() >= Duration::from_secs(5) {
            if let Some(active_discovery) = discovery.as_ref() {
                if !active_discovery.is_active() {
                    eprintln!("BlueZ discovery stopped; reconnecting");
                    ble_generation = ble_generation.wrapping_add(1);
                    discovery = None;
                    if let Some(mut scanner) = ble_scanner.take() {
                        scanner.stop();
                    }
                    last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                    receivers.bluetooth = BluetoothStats {
                        enabled: true,
                        ..BluetoothStats::default()
                    };
                    publisher.publish(&state, &receivers)?;
                }
            }
            last_discovery_check = Instant::now();
        }

        match events_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::BleReading {
                generation,
                reading,
            }) if generation == ble_generation && receivers.bluetooth.enabled => {
                state.update(*reading);
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::BluetoothStats {
                generation,
                device_count,
                manufacturer_data_count,
            }) if generation == ble_generation && receivers.bluetooth.enabled => {
                receivers.bluetooth.device_count = device_count;
                receivers.bluetooth.manufacturer_data_count = manufacturer_data_count;
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Reading {
                generation,
                reading,
            }) if generation == rtl433_generation && receivers.rtl433.enabled => {
                state.update(*reading);
                receivers.rtl433.frame_count = receivers.rtl433.frame_count.saturating_add(1);
                receivers.rtl433.ready = true;
                receivers.rtl433.recent_valid_activity = true;
                receivers.rtl433.last_error.clear();
                rtl433_failures = 0;
                rtl433_retry_seconds = 0;
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Report {
                generation,
                fsk_frames,
                decoder_events,
                decoded_ok,
            }) if generation == rtl433_generation && receivers.rtl433.enabled => {
                last_rtl433_heartbeat = Instant::now();
                receivers.rtl433.ready = true;
                receivers.rtl433.stats_heartbeat_count =
                    receivers.rtl433.stats_heartbeat_count.saturating_add(1);
                receivers.rtl433.fsk_activity_count = receivers
                    .rtl433
                    .fsk_activity_count
                    .saturating_add(fsk_frames);
                receivers.rtl433.decoder_event_count = receivers
                    .rtl433
                    .decoder_event_count
                    .saturating_add(decoder_events);
                receivers.rtl433.rejected_frame_count = receivers
                    .rtl433
                    .rejected_frame_count
                    .saturating_add(decoder_events.saturating_sub(decoded_ok));
                receivers.rtl433.recent_fsk_activity = fsk_frames > 0;
                receivers.rtl433.recent_valid_activity = decoded_ok > 0;
                receivers.rtl433.last_error.clear();
                rtl433_failures = 0;
                rtl433_retry_seconds = 0;
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::Rtl433Exited {
                generation,
                success,
            }) if generation == rtl433_generation && receivers.rtl433.enabled => {
                if let Some(process) = rtl433_process.as_mut() {
                    process.mark_exited();
                }
                rtl433_process = None;
                receivers.rtl433.active = false;
                receivers.rtl433.ready = false;
                receivers.rtl433.recent_fsk_activity = false;
                receivers.rtl433.recent_valid_activity = false;
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
            Ok(Event::BleReading { .. })
            | Ok(Event::BluetoothStats { .. })
            | Ok(Event::Rtl433Reading { .. })
            | Ok(Event::Rtl433Report { .. })
            | Ok(Event::Rtl433Exited { .. }) => {}
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
            Ok(Event::SetReceiverMode(receiver_mode)) => {
                state.set_receiver_mode(receiver_mode)?;
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::BleScannerFailed { generation, error })
                if generation == ble_generation && receivers.bluetooth.enabled =>
            {
                eprintln!("HCI scanner stopped: {error}");
                ble_generation = ble_generation.wrapping_add(1);
                discovery = None;
                if let Some(mut scanner) = ble_scanner.take() {
                    scanner.stop();
                }
                last_adapter_attempt = Instant::now() - ADAPTER_RETRY_INTERVAL;
                receivers.bluetooth = BluetoothStats {
                    enabled: true,
                    ..BluetoothStats::default()
                };
                publisher.publish(&state, &receivers)?;
            }
            Ok(Event::BleScannerFailed { .. }) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("TPMS event channel disconnected").into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if receivers.rtl433.enabled
            && rtl433_heartbeat_expired(rtl433_process.is_some(), last_rtl433_heartbeat.elapsed())
        {
            eprintln!("rtl_433 stats heartbeat timed out; restarting receiver");
            rtl433_generation = rtl433_generation.wrapping_add(1);
            let stop_error = if let Some(mut process) = rtl433_process.take() {
                process.stop().err()
            } else {
                None
            };
            receivers.rtl433.active = false;
            receivers.rtl433.ready = false;
            receivers.rtl433.recent_fsk_activity = false;
            receivers.rtl433.recent_valid_activity = false;
            receivers.rtl433.last_error = stop_error
                .map(|error| format!("rtl_433 heartbeat timed out; stop failed: {error}"))
                .unwrap_or_else(|| "rtl_433 stats heartbeat timed out".to_owned());
            rtl433_failures = rtl433_failures.saturating_add(1);
            rtl433_retry_seconds = retry_delay_seconds(rtl433_failures);
            last_rtl433_attempt = Instant::now();
            publisher.publish(&state, &receivers)?;
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
    use super::{
        adapter_retry_due, rtl433_heartbeat_expired, ADAPTER_RETRY_INTERVAL,
        RTL433_HEARTBEAT_TIMEOUT,
    };
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

    #[test]
    fn rtl433_is_restarted_only_when_an_active_process_loses_its_heartbeat() {
        assert!(!rtl433_heartbeat_expired(
            true,
            RTL433_HEARTBEAT_TIMEOUT - Duration::from_secs(1)
        ));
        assert!(rtl433_heartbeat_expired(true, RTL433_HEARTBEAT_TIMEOUT));
        assert!(!rtl433_heartbeat_expired(false, Duration::MAX));
    }
}
