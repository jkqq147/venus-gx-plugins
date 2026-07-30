use std::{collections::HashMap, sync::mpsc::Sender};

use zbus::blocking::Connection;

use crate::{
    bus_item::{BusItem, BusItemHandle},
    event::Event,
    state::{unix_time, Reading, TpmsState, Wheel, DISCOVERED_LIMIT},
};

const UNASSIGNED: &str = "unassigned";

pub struct VenusPublisher {
    connection: Connection,
    handles: HashMap<String, BusItemHandle>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BluetoothStats {
    pub adapter_index: Option<u16>,
    pub active: bool,
    pub device_count: usize,
    pub manufacturer_data_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Rtl433Stats {
    pub active: bool,
    pub frame_count: u32,
    pub fsk_activity_count: u32,
    pub decoder_event_count: u32,
    pub rejected_frame_count: u32,
    pub stats_heartbeat_count: u32,
    pub recent_fsk_activity: bool,
    pub last_error: String,
}

#[derive(Debug, Clone, Default)]
pub struct ReceiverStats {
    pub bluetooth: BluetoothStats,
    pub rtl433: Rtl433Stats,
}

impl VenusPublisher {
    pub fn new(
        connection: Connection,
        service_name: &str,
        events: Sender<Event>,
    ) -> zbus::Result<Self> {
        let mut publisher = Self {
            connection,
            handles: HashMap::new(),
        };

        for (path, value) in [
            ("/Mgmt/ProcessName", "venus-tpms"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "bluez + rtl_433"),
            ("/ProductName", "TPMS"),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/HardwareVersion", ""),
            ("/StatusText", "Starting"),
            ("/Overview", "FL --  FR -- / RL --  RR --"),
            ("/Bluetooth/StatusText", "Starting"),
            ("/Bluetooth/Adapter", ""),
            ("/Bluetooth/ReceiverStatus", "Starting"),
            ("/Receivers/Rtl433/StatusText", "Starting"),
            ("/Receivers/Rtl433/LastError", ""),
        ] {
            publisher.add(path, BusItem::string(value))?;
        }
        for (path, value) in [
            ("/DeviceInstance", 0),
            ("/ProductId", 0),
            ("/Connected", 1),
            ("/Status", 0),
            ("/DiscoveredCount", 0),
            ("/Bluetooth/Discovering", 0),
            ("/Bluetooth/DeviceCount", 0),
            ("/Bluetooth/ManufacturerDataCount", 0),
            ("/Receivers/Rtl433/Active", 0),
            ("/Receivers/Rtl433/FrameCount", 0),
            ("/Receivers/Rtl433/FskActivityCount", 0),
            ("/Receivers/Rtl433/DecoderEventCount", 0),
            ("/Receivers/Rtl433/RejectedFrameCount", 0),
            ("/Receivers/Rtl433/StatsHeartbeatCount", 0),
        ] {
            publisher.add(path, BusItem::i32(value))?;
        }

        for wheel in Wheel::ALL {
            let root = format!("/Slots/{}", wheel.key());
            publisher.add(&format!("{root}/Wheel"), BusItem::string(wheel.key()))?;
            publisher.add(&format!("{root}/Name"), BusItem::string(wheel.label()))?;
            publisher.add(&format!("{root}/SensorId"), BusItem::string(""))?;
            for suffix in [
                "Pressure",
                "Temperature",
                "Battery",
                "BatteryVoltage",
                "Rssi",
                "Signal",
                "Snr",
                "Noise",
                "LastSeen",
            ] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::invalid())?;
            }
            for (suffix, value) in [
                ("Source", ""),
                ("Protocol", ""),
                ("AlarmText", ""),
                ("ManufacturerData", ""),
                ("BatteryDisplay", "--"),
                ("SignalDisplay", "--"),
                ("RssiDisplay", "--"),
                ("State", UNASSIGNED),
                ("StateText", "Unassigned"),
                ("Summary", "Unassigned"),
                ("DeviceListValue", "--"),
                ("PressureDisplay", "--"),
                ("TemperatureDisplay", "--"),
            ] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::string(value))?;
            }
        }

        for index in 0..DISCOVERED_LIMIT {
            let root = format!("/Discovered/{index}");
            for suffix in [
                "Name",
                "SensorId",
                "Source",
                "Protocol",
                "AlarmText",
                "BatteryDisplay",
                "SignalDisplay",
                "ManufacturerData",
            ] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::string(""))?;
            }
            let sender = events.clone();
            publisher.add(
                &format!("{root}/AssignedWheel"),
                BusItem::writable_string(UNASSIGNED, move |value| {
                    let wheel = if value == UNASSIGNED {
                        None
                    } else if let Some(wheel) = Wheel::parse(&value) {
                        Some(wheel)
                    } else {
                        return 2;
                    };
                    sender.send(Event::Assign { index, wheel }).map_or(2, |_| 0)
                }),
            )?;
            for suffix in [
                "Pressure",
                "Temperature",
                "Battery",
                "BatteryVoltage",
                "Rssi",
                "Signal",
                "Snr",
                "Noise",
                "LastSeen",
            ] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::invalid())?;
            }
            for (suffix, value) in [
                ("PressureDisplay", "--"),
                ("TemperatureDisplay", "--"),
                ("RssiDisplay", "--"),
            ] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::string(value))?;
            }
        }

        publisher.connection.request_name(service_name)?;
        Ok(publisher)
    }

    pub fn publish(&self, state: &TpmsState, receivers: &ReceiverStats) -> zbus::Result<()> {
        let bluetooth = receivers.bluetooth;
        let status = if bluetooth.active || receivers.rtl433.active {
            "Scanning"
        } else {
            "No TPMS receiver"
        };
        self.string("/StatusText", status)?;
        self.string(
            "/Bluetooth/StatusText",
            if bluetooth.active {
                "Scanning"
            } else {
                "No Bluetooth adapter"
            },
        )?;
        self.string(
            "/Bluetooth/Adapter",
            &bluetooth
                .adapter_index
                .map(|index| format!("/org/bluez/hci{index}"))
                .unwrap_or_default(),
        )?;
        self.string(
            "/Bluetooth/ReceiverStatus",
            if !bluetooth.active {
                "Unavailable"
            } else if bluetooth.device_count > 0 {
                "Receiving"
            } else {
                "Listening"
            },
        )?;
        self.i32("/Bluetooth/Discovering", i32::from(bluetooth.active))?;
        self.string(
            "/Receivers/Rtl433/StatusText",
            if receivers.rtl433.active {
                if receivers.rtl433.frame_count > 0 {
                    "Receiving"
                } else if receivers.rtl433.recent_fsk_activity {
                    "RF activity"
                } else {
                    "Listening"
                }
            } else {
                "Unavailable"
            },
        )?;
        self.string("/Receivers/Rtl433/LastError", &receivers.rtl433.last_error)?;
        self.i32(
            "/Receivers/Rtl433/Active",
            i32::from(receivers.rtl433.active),
        )?;
        self.i32(
            "/Receivers/Rtl433/FrameCount",
            receivers.rtl433.frame_count.min(i32::MAX as u32) as i32,
        )?;
        self.i32(
            "/Receivers/Rtl433/FskActivityCount",
            receivers.rtl433.fsk_activity_count.min(i32::MAX as u32) as i32,
        )?;
        self.i32(
            "/Receivers/Rtl433/DecoderEventCount",
            receivers.rtl433.decoder_event_count.min(i32::MAX as u32) as i32,
        )?;
        self.i32(
            "/Receivers/Rtl433/RejectedFrameCount",
            receivers.rtl433.rejected_frame_count.min(i32::MAX as u32) as i32,
        )?;
        self.i32(
            "/Receivers/Rtl433/StatsHeartbeatCount",
            receivers.rtl433.stats_heartbeat_count.min(i32::MAX as u32) as i32,
        )?;

        let discovered = state.discovered();
        self.i32("/DiscoveredCount", discovered.len() as i32)?;
        self.i32("/Bluetooth/DeviceCount", bluetooth.device_count as i32)?;
        self.i32(
            "/Bluetooth/ManufacturerDataCount",
            bluetooth.manufacturer_data_count as i32,
        )?;
        for index in 0..DISCOVERED_LIMIT {
            let root = format!("/Discovered/{index}");
            let Some(reading) = discovered.get(index) else {
                self.clear_discovered(&root)?;
                continue;
            };
            self.string(&format!("{root}/Name"), &reading.display_name())?;
            self.string(&format!("{root}/SensorId"), &reading.sensor_id)?;
            self.string(
                &format!("{root}/AssignedWheel"),
                state
                    .assigned_wheel(&reading.sensor_key)
                    .map(Wheel::key)
                    .unwrap_or(UNASSIGNED),
            )?;
            self.publish_reading(&root, reading)?;
        }

        let now = unix_time();
        let mut overview = Vec::new();
        for wheel in Wheel::ALL {
            overview.push(self.publish_wheel(state, wheel, now)?);
        }
        self.string(
            "/Overview",
            &format!(
                "FL {}  FR {} / RL {}  RR {}",
                overview[0], overview[1], overview[2], overview[3]
            ),
        )
    }

    fn publish_wheel(&self, state: &TpmsState, wheel: Wheel, now: i64) -> zbus::Result<String> {
        let root = format!("/Slots/{}", wheel.key());
        let sensor_id = state.binding(wheel);
        if sensor_id.is_empty() {
            self.clear_wheel(&root, wheel, "Unassigned", UNASSIGNED, "--")?;
            return Ok("--".to_owned());
        }
        let Some(reading) = state.reading_for_wheel(wheel) else {
            self.clear_wheel(&root, wheel, "Waiting", "waiting", "wait")?;
            self.string(&format!("{root}/SensorId"), sensor_id)?;
            return Ok("wait".to_owned());
        };
        let stale = now - reading.last_seen > reading.stale_after_seconds();
        let (state_key, state_text) = if stale {
            ("stale", "Stale")
        } else if reading.alarm.is_some() {
            ("alarm", "Alarm")
        } else {
            ("ok", "OK")
        };
        self.string(&format!("{root}/Name"), &reading.display_name())?;
        self.string(&format!("{root}/SensorId"), &reading.sensor_id)?;
        self.publish_reading(&root, reading)?;
        self.string(&format!("{root}/State"), state_key)?;
        self.string(&format!("{root}/StateText"), state_text)?;
        let temperature = reading
            .temperature_c
            .map(|value| format!("{value:.1} C"))
            .unwrap_or_else(|| "--".to_owned());
        let battery = battery_display(reading);
        self.string(
            &format!("{root}/Summary"),
            &format!(
                "{}: {:.2} bar, {}, {}",
                state_text, reading.pressure_bar, temperature, battery
            ),
        )?;
        let pressure = format!(
            "{:.2}{}",
            reading.pressure_bar,
            if stale { "*" } else { "" }
        );
        self.string(&format!("{root}/DeviceListValue"), &pressure)?;
        self.string(&format!("{root}/PressureDisplay"), &pressure)?;
        Ok(pressure)
    }

    fn clear_discovered(&self, root: &str) -> zbus::Result<()> {
        for suffix in [
            "Name",
            "SensorId",
            "Source",
            "Protocol",
            "AlarmText",
            "ManufacturerData",
        ] {
            self.string(&format!("{root}/{suffix}"), "")?;
        }
        self.string(&format!("{root}/AssignedWheel"), UNASSIGNED)?;
        for suffix in [
            "Pressure",
            "Temperature",
            "Battery",
            "BatteryVoltage",
            "Rssi",
            "Signal",
            "Snr",
            "Noise",
            "LastSeen",
        ] {
            self.invalid(&format!("{root}/{suffix}"))?;
        }
        for suffix in [
            "PressureDisplay",
            "TemperatureDisplay",
            "RssiDisplay",
            "BatteryDisplay",
            "SignalDisplay",
        ] {
            self.string(&format!("{root}/{suffix}"), "--")?;
        }
        Ok(())
    }

    fn clear_wheel(
        &self,
        root: &str,
        wheel: Wheel,
        state_text: &str,
        state: &str,
        display: &str,
    ) -> zbus::Result<()> {
        self.string(&format!("{root}/Name"), wheel.label())?;
        self.string(&format!("{root}/SensorId"), "")?;
        for suffix in [
            "Pressure",
            "Temperature",
            "Battery",
            "BatteryVoltage",
            "Rssi",
            "Signal",
            "Snr",
            "Noise",
            "LastSeen",
        ] {
            self.invalid(&format!("{root}/{suffix}"))?;
        }
        for suffix in ["Source", "Protocol", "AlarmText", "ManufacturerData"] {
            self.string(&format!("{root}/{suffix}"), "")?;
        }
        for suffix in ["BatteryDisplay", "SignalDisplay", "RssiDisplay"] {
            self.string(&format!("{root}/{suffix}"), "--")?;
        }
        self.string(&format!("{root}/State"), state)?;
        self.string(&format!("{root}/StateText"), state_text)?;
        self.string(&format!("{root}/Summary"), state_text)?;
        self.string(&format!("{root}/DeviceListValue"), display)?;
        self.string(&format!("{root}/PressureDisplay"), display)?;
        self.string(&format!("{root}/TemperatureDisplay"), "--")
    }

    fn publish_reading(&self, root: &str, reading: &Reading) -> zbus::Result<()> {
        self.string(&format!("{root}/Source"), reading.source.label())?;
        self.string(&format!("{root}/Protocol"), &reading.protocol)?;
        self.string(
            &format!("{root}/AlarmText"),
            reading.alarm.as_deref().unwrap_or(""),
        )?;
        self.f64(&format!("{root}/Pressure"), reading.pressure_bar)?;
        self.string(
            &format!("{root}/PressureDisplay"),
            &format!("{:.2}", reading.pressure_bar),
        )?;
        self.optional_f64(&format!("{root}/Temperature"), reading.temperature_c)?;
        self.string(
            &format!("{root}/TemperatureDisplay"),
            &reading
                .temperature_c
                .map(|value| format!("{value:.1}C"))
                .unwrap_or_else(|| "--".to_owned()),
        )?;
        self.optional_i32(&format!("{root}/Battery"), reading.battery_percent)?;
        self.optional_i32(&format!("{root}/BatteryVoltage"), reading.battery_mv)?;
        self.string(&format!("{root}/BatteryDisplay"), &battery_display(reading))?;
        self.optional_f64(&format!("{root}/Signal"), reading.signal_value)?;
        self.optional_f64(&format!("{root}/Snr"), reading.snr_db)?;
        self.optional_f64(&format!("{root}/Noise"), reading.noise_db)?;
        if reading.signal_unit == "dBm" {
            self.optional_i32(
                &format!("{root}/Rssi"),
                reading.signal_value.map(|value| value.round() as i32),
            )?;
        } else {
            self.invalid(&format!("{root}/Rssi"))?;
        }
        let signal = reading
            .signal_value
            .map(|value| format!("{value:.1} {}", reading.signal_unit))
            .unwrap_or_else(|| "--".to_owned());
        self.string(&format!("{root}/SignalDisplay"), &signal)?;
        self.string(&format!("{root}/RssiDisplay"), &signal)?;
        self.i32(&format!("{root}/LastSeen"), reading.last_seen as i32)?;
        self.string(
            &format!("{root}/ManufacturerData"),
            reading.manufacturer_data_hex.as_deref().unwrap_or(""),
        )
    }

    fn add(&mut self, path: &str, item: BusItem) -> zbus::Result<()> {
        let handle = item.handle();
        self.connection.object_server().at(path, item)?;
        self.handles.insert(path.to_owned(), handle);
        Ok(())
    }

    fn handle(&self, path: &str) -> &BusItemHandle {
        self.handles
            .get(path)
            .unwrap_or_else(|| panic!("missing D-Bus path {path}"))
    }

    fn string(&self, path: &str, value: &str) -> zbus::Result<()> {
        self.handle(path)
            .set_string(&self.connection, path, value.to_owned())
    }

    fn i32(&self, path: &str, value: i32) -> zbus::Result<()> {
        self.handle(path).set_i32(&self.connection, path, value)
    }

    fn f64(&self, path: &str, value: f64) -> zbus::Result<()> {
        self.handle(path).set_f64(&self.connection, path, value)
    }

    fn optional_i32(&self, path: &str, value: Option<i32>) -> zbus::Result<()> {
        match value {
            Some(value) => self.i32(path, value),
            None => self.invalid(path),
        }
    }

    fn optional_f64(&self, path: &str, value: Option<f64>) -> zbus::Result<()> {
        match value {
            Some(value) => self.f64(path, value),
            None => self.invalid(path),
        }
    }

    fn invalid(&self, path: &str) -> zbus::Result<()> {
        self.handle(path).set_invalid(&self.connection, path)
    }
}

fn battery_display(reading: &Reading) -> String {
    if let Some(percent) = reading.battery_percent {
        format!("{percent}%")
    } else if let Some(millivolts) = reading.battery_mv {
        format!("{:.2} V", millivolts as f64 / 1000.0)
    } else {
        "--".to_owned()
    }
}
