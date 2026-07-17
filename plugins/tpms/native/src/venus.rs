use std::{collections::HashMap, sync::mpsc::Sender};

use zbus::blocking::Connection;

use crate::{
    bus_item::{BusItem, BusItemHandle},
    event::Event,
    state::{unix_time, TpmsState, Wheel, DISCOVERED_LIMIT},
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
            ("/Mgmt/ProcessName", "venus-tpms-ble"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "bluez"),
            ("/ProductName", "TPMS BLE"),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/HardwareVersion", ""),
            ("/StatusText", "Starting"),
            ("/Overview", "FL --  FR -- / RL --  RR --"),
            ("/Bluetooth/StatusText", "Starting"),
            ("/Bluetooth/Adapter", ""),
            ("/Bluetooth/ReceiverStatus", "Starting"),
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
        ] {
            publisher.add(path, BusItem::i32(value))?;
        }

        for wheel in Wheel::ALL {
            let root = format!("/Slots/{}", wheel.key());
            publisher.add(&format!("{root}/Wheel"), BusItem::string(wheel.key()))?;
            publisher.add(&format!("{root}/Name"), BusItem::string(wheel.label()))?;
            publisher.add(&format!("{root}/SensorId"), BusItem::string(""))?;
            for suffix in ["Pressure", "Temperature", "Battery", "Rssi", "LastSeen"] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::invalid())?;
            }
            for (suffix, value) in [
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
            for suffix in ["Name", "SensorId", "ManufacturerData"] {
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
            for suffix in ["Pressure", "Temperature", "Battery", "Rssi", "LastSeen"] {
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

    pub fn publish(
        &self,
        state: &TpmsState,
        stale_seconds: i64,
        bluetooth: BluetoothStats,
    ) -> zbus::Result<()> {
        let status = if bluetooth.active {
            "Scanning"
        } else {
            "No Bluetooth adapter"
        };
        self.string("/StatusText", status)?;
        self.string("/Bluetooth/StatusText", status)?;
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
                    .assigned_wheel(&reading.sensor_id)
                    .map(Wheel::key)
                    .unwrap_or(UNASSIGNED),
            )?;
            self.f64(&format!("{root}/Pressure"), reading.pressure_bar)?;
            self.string(
                &format!("{root}/PressureDisplay"),
                &format!("{:.2}", reading.pressure_bar),
            )?;
            self.f64(&format!("{root}/Temperature"), reading.temperature_c)?;
            self.string(
                &format!("{root}/TemperatureDisplay"),
                &format!("{:.1}C", reading.temperature_c),
            )?;
            self.i32(&format!("{root}/Battery"), reading.battery_percent)?;
            self.i32(&format!("{root}/Rssi"), reading.rssi)?;
            self.string(
                &format!("{root}/RssiDisplay"),
                &format!("{}dB", reading.rssi),
            )?;
            self.i32(&format!("{root}/LastSeen"), reading.last_seen as i32)?;
            self.string(
                &format!("{root}/ManufacturerData"),
                &reading.manufacturer_data_hex,
            )?;
        }

        let now = unix_time();
        let mut overview = Vec::new();
        for wheel in Wheel::ALL {
            overview.push(self.publish_wheel(state, wheel, now, stale_seconds)?);
        }
        self.string(
            "/Overview",
            &format!(
                "FL {}  FR {} / RL {}  RR {}",
                overview[0], overview[1], overview[2], overview[3]
            ),
        )
    }

    fn publish_wheel(
        &self,
        state: &TpmsState,
        wheel: Wheel,
        now: i64,
        stale_seconds: i64,
    ) -> zbus::Result<String> {
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
        let stale = now - reading.last_seen > stale_seconds;
        let state_key = if stale { "stale" } else { "ok" };
        let state_text = if stale { "Stale" } else { "OK" };
        self.string(&format!("{root}/Name"), &reading.display_name())?;
        self.string(&format!("{root}/SensorId"), sensor_id)?;
        self.f64(&format!("{root}/Pressure"), reading.pressure_bar)?;
        self.f64(&format!("{root}/Temperature"), reading.temperature_c)?;
        self.i32(&format!("{root}/Battery"), reading.battery_percent)?;
        self.i32(&format!("{root}/Rssi"), reading.rssi)?;
        self.i32(&format!("{root}/LastSeen"), reading.last_seen as i32)?;
        self.string(&format!("{root}/State"), state_key)?;
        self.string(&format!("{root}/StateText"), state_text)?;
        self.string(
            &format!("{root}/Summary"),
            &format!(
                "{}: {:.2} bar, {:.1} C, {}%",
                state_text, reading.pressure_bar, reading.temperature_c, reading.battery_percent
            ),
        )?;
        let pressure = format!(
            "{:.2}{}",
            reading.pressure_bar,
            if stale { "*" } else { "" }
        );
        self.string(&format!("{root}/DeviceListValue"), &pressure)?;
        self.string(&format!("{root}/PressureDisplay"), &pressure)?;
        self.string(
            &format!("{root}/TemperatureDisplay"),
            &format!("{:.1}C", reading.temperature_c),
        )?;
        Ok(pressure)
    }

    fn clear_discovered(&self, root: &str) -> zbus::Result<()> {
        for suffix in ["Name", "SensorId", "ManufacturerData"] {
            self.string(&format!("{root}/{suffix}"), "")?;
        }
        self.string(&format!("{root}/AssignedWheel"), UNASSIGNED)?;
        for suffix in ["Pressure", "Temperature", "Battery", "Rssi", "LastSeen"] {
            self.invalid(&format!("{root}/{suffix}"))?;
        }
        for suffix in ["PressureDisplay", "TemperatureDisplay", "RssiDisplay"] {
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
        for suffix in ["Pressure", "Temperature", "Battery", "Rssi", "LastSeen"] {
            self.invalid(&format!("{root}/{suffix}"))?;
        }
        self.string(&format!("{root}/State"), state)?;
        self.string(&format!("{root}/StateText"), state_text)?;
        self.string(&format!("{root}/Summary"), state_text)?;
        self.string(&format!("{root}/DeviceListValue"), display)?;
        self.string(&format!("{root}/PressureDisplay"), display)?;
        self.string(&format!("{root}/TemperatureDisplay"), "--")
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

    fn invalid(&self, path: &str) -> zbus::Result<()> {
        self.handle(path).set_invalid(&self.connection, path)
    }
}
