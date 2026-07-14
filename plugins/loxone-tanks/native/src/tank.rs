use std::sync::mpsc::Sender;

use zbus::blocking::Connection;

use crate::{
    bus_item::{BusItem, BusItems},
    config::Config,
    probe::TankKind,
    publisher::Command,
};

pub struct TankServices {
    fresh: TankService,
    gray: TankService,
    black: TankService,
}

impl TankServices {
    pub fn new(config: &Config, commands: Sender<Command>) -> zbus::Result<Self> {
        Ok(Self {
            fresh: TankService::new(
                TankKind::Fresh,
                config.tanks.fresh.capacity_liters,
                40,
                commands.clone(),
            )?,
            gray: TankService::new(
                TankKind::Gray,
                config.tanks.gray.capacity_liters,
                41,
                commands.clone(),
            )?,
            black: TankService::new(
                TankKind::Black,
                config.tanks.black.capacity_liters,
                42,
                commands,
            )?,
        })
    }

    pub fn set_capacity(&mut self, tank: TankKind, capacity_liters: f64) -> zbus::Result<()> {
        self.service_mut(tank).set_capacity(capacity_liters)
    }

    pub fn register(&self) -> zbus::Result<()> {
        self.fresh.register()?;
        self.gray.register()?;
        self.black.register()
    }

    pub fn set_level(&mut self, tank: TankKind, level: f64) -> zbus::Result<f64> {
        self.service_mut(tank).set_level(level)
    }

    pub fn set_disconnected(&mut self) -> zbus::Result<()> {
        self.fresh.set_disconnected()?;
        self.gray.set_disconnected()?;
        self.black.set_disconnected()
    }

    fn service_mut(&mut self, tank: TankKind) -> &mut TankService {
        match tank {
            TankKind::Fresh => &mut self.fresh,
            TankKind::Gray => &mut self.gray,
            TankKind::Black => &mut self.black,
        }
    }
}

struct TankService {
    connection: Connection,
    items: BusItems,
    service_name: &'static str,
    capacity_liters: f64,
    level: Option<f64>,
}

impl TankService {
    fn new(
        tank: TankKind,
        capacity_liters: f64,
        device_instance: i32,
        commands: Sender<Command>,
    ) -> zbus::Result<Self> {
        let connection = Connection::system()?;
        let mut service = Self {
            connection,
            items: BusItems::default(),
            service_name: service_name(tank),
            capacity_liters,
            level: None,
        };
        service
            .connection
            .object_server()
            .at("/", service.items.root())?;
        for (path, value) in [
            ("/Mgmt/ProcessName", "venus-loxone-tanks"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "Loxone Miniserver"),
            ("/ProductName", product_name(tank)),
            ("/CustomName", custom_name(tank)),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/HardwareVersion", "Loxone"),
        ] {
            service.add(path, BusItem::string(value))?;
        }
        for (path, value) in [
            ("/DeviceInstance", device_instance),
            ("/ProductId", 0),
            ("/Connected", 0),
            ("/FluidType", fluid_type(tank)),
        ] {
            service.add(path, BusItem::i32(value))?;
        }
        service.add("/Level", BusItem::invalid())?;
        service.add(
            "/Capacity",
            BusItem::writable_f64(liters_to_cubic_meters(capacity_liters), move |value| {
                let Some(capacity_liters) = capacity_liters_from_bus(value) else {
                    return 2;
                };
                commands
                    .send(Command::SetTankCapacity(tank, capacity_liters))
                    .map_or(2, |_| 0)
            }),
        )?;
        service.add("/Remaining", BusItem::invalid())?;
        Ok(service)
    }

    fn register(&self) -> zbus::Result<()> {
        self.connection.request_name(self.service_name)?;
        Ok(())
    }

    fn set_capacity(&mut self, capacity_liters: f64) -> zbus::Result<()> {
        self.capacity_liters = capacity_liters;
        self.f64("/Capacity", liters_to_cubic_meters(capacity_liters))?;
        match (capacity_liters > 0.0, self.level) {
            (true, Some(level)) => {
                self.f64("/Remaining", remaining_cubic_meters(capacity_liters, level))
            }
            _ => self.invalid("/Remaining"),
        }
    }

    fn set_level(&mut self, level: f64) -> zbus::Result<f64> {
        let level = normalize_level(level);
        self.level = Some(level);
        self.i32("/Connected", 1)?;
        self.f64("/Level", level)?;
        if self.capacity_liters > 0.0 {
            self.f64(
                "/Remaining",
                remaining_cubic_meters(self.capacity_liters, level),
            )?;
        } else {
            self.invalid("/Remaining")?;
        }
        Ok(level)
    }

    fn set_disconnected(&mut self) -> zbus::Result<()> {
        self.level = None;
        self.i32("/Connected", 0)?;
        self.invalid("/Level")?;
        self.invalid("/Remaining")
    }

    fn add(&mut self, path: &'static str, item: BusItem) -> zbus::Result<()> {
        let handle = item.handle();
        self.connection.object_server().at(path, item)?;
        self.items.insert(path, handle);
        Ok(())
    }

    fn handle(&self, path: &'static str) -> crate::bus_item::BusItemHandle {
        self.items.handle(path)
    }

    fn i32(&self, path: &'static str, value: i32) -> zbus::Result<()> {
        self.handle(path).set_i32(&self.connection, path, value)
    }

    fn f64(&self, path: &'static str, value: f64) -> zbus::Result<()> {
        self.handle(path).set_f64(&self.connection, path, value)
    }

    fn invalid(&self, path: &'static str) -> zbus::Result<()> {
        self.handle(path).set_invalid(&self.connection, path)
    }
}

fn service_name(tank: TankKind) -> &'static str {
    match tank {
        TankKind::Fresh => "com.victronenergy.tank.loxone_fresh",
        TankKind::Gray => "com.victronenergy.tank.loxone_gray",
        TankKind::Black => "com.victronenergy.tank.loxone_black",
    }
}

fn product_name(tank: TankKind) -> &'static str {
    match tank {
        TankKind::Fresh => "Loxone Fresh Water",
        TankKind::Gray => "Loxone Gray Water",
        TankKind::Black => "Loxone Black Water",
    }
}

fn custom_name(tank: TankKind) -> &'static str {
    match tank {
        TankKind::Fresh => "Fw Tank",
        TankKind::Gray => "Gw Tank",
        TankKind::Black => "Bw Tank",
    }
}

fn fluid_type(tank: TankKind) -> i32 {
    match tank {
        TankKind::Fresh => 1,
        TankKind::Gray => 2,
        TankKind::Black => 5,
    }
}

fn normalize_level(level: f64) -> f64 {
    level.clamp(0.0, 100.0)
}

fn liters_to_cubic_meters(liters: f64) -> f64 {
    liters / 1_000.0
}

fn capacity_liters_from_bus(cubic_meters: f64) -> Option<f64> {
    let liters = cubic_meters * 1_000.0;
    (liters.is_finite() && (0.0..=100_000.0).contains(&liters)).then_some(liters)
}

fn remaining_cubic_meters(capacity_liters: f64, level: f64) -> f64 {
    liters_to_cubic_meters(capacity_liters) * normalize_level(level) / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tank_values_use_venus_cubic_meter_units() {
        assert_eq!(normalize_level(-1.0), 0.0);
        assert_eq!(normalize_level(101.0), 100.0);
        assert_eq!(liters_to_cubic_meters(180.0), 0.18);
        assert_eq!(capacity_liters_from_bus(0.18), Some(180.0));
        assert_eq!(remaining_cubic_meters(180.0, 50.0), 0.09);
        assert_eq!(capacity_liters_from_bus(-0.1), None);
        assert_eq!(capacity_liters_from_bus(f64::NAN), None);
    }

    #[test]
    fn fluid_types_match_venus_os() {
        assert_eq!(fluid_type(TankKind::Fresh), 1);
        assert_eq!(fluid_type(TankKind::Gray), 2);
        assert_eq!(fluid_type(TankKind::Black), 5);
    }

    #[test]
    fn custom_names_match_the_fixed_loxone_sensor_names() {
        assert_eq!(custom_name(TankKind::Fresh), "Fw Tank");
        assert_eq!(custom_name(TankKind::Gray), "Gw Tank");
        assert_eq!(custom_name(TankKind::Black), "Bw Tank");
        assert_eq!(product_name(TankKind::Fresh), "Loxone Fresh Water");
    }
}
