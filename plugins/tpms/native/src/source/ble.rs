use std::{sync::mpsc::Sender, thread};

use crate::{
    event::Event,
    linux,
    state::{unix_time, Reading, SourceKind},
};

pub fn spawn(adapter_index: u16, events: Sender<Event>) {
    thread::spawn(move || {
        let reading_events = events.clone();
        if let Err(error) = linux::run_with_handlers(
            adapter_index,
            move |observation| {
                let parsed = observation.reading;
                let sensor_key = parsed.sensor_id_hex.clone();
                let reading = Reading {
                    sensor_key,
                    sensor_id: parsed.sensor_id_hex,
                    source: SourceKind::Ble,
                    protocol: "BLE TPMS".to_owned(),
                    name: observation.local_name.unwrap_or_default(),
                    pressure_bar: parsed.pressure_bar as f64,
                    temperature_c: Some(parsed.temperature_c as f64),
                    battery_percent: Some(parsed.battery_percent as i32),
                    battery_mv: None,
                    alarm: (parsed.alarm != 0).then(|| parsed.alarm.to_string()),
                    signal_value: Some(observation.rssi as f64),
                    signal_unit: "dBm".to_owned(),
                    snr_db: None,
                    noise_db: None,
                    last_seen: unix_time(),
                    manufacturer_data_hex: Some(observation.manufacturer_data_hex),
                };
                let _ = reading_events.send(Event::Reading(Box::new(reading)));
            },
            {
                let stats_events = events.clone();
                move |stats| {
                    let _ = stats_events.send(Event::BluetoothStats {
                        device_count: stats.device_count,
                        manufacturer_data_count: stats.manufacturer_data_count,
                    });
                }
            },
        ) {
            let _ = events.send(Event::ScannerFailed(error.to_string()));
        }
    });
}
