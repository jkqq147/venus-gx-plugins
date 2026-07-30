use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread::{self, JoinHandle},
};

use crate::{
    event::Event,
    linux,
    state::{unix_time, Reading, SourceKind},
};

pub struct BleScanner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BleScanner {
    pub fn start(adapter_index: u16, generation: u64, events: Sender<Event>) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("tpms-ble".to_owned())
            .spawn(move || {
                let reading_events = events.clone();
                if let Err(error) = linux::run_with_handlers_until(
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
                        let _ = reading_events.send(Event::BleReading {
                            generation,
                            reading: Box::new(reading),
                        });
                    },
                    {
                        let stats_events = events.clone();
                        move |stats| {
                            let _ = stats_events.send(Event::BluetoothStats {
                                generation,
                                device_count: stats.device_count,
                                manufacturer_data_count: stats.manufacturer_data_count,
                            });
                        }
                    },
                    || thread_stop.load(Ordering::Relaxed),
                ) {
                    let _ = events.send(Event::BleScannerFailed {
                        generation,
                        error: error.to_string(),
                    });
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for BleScanner {
    fn drop(&mut self) {
        self.stop();
    }
}
