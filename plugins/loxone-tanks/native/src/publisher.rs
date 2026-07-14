use std::sync::mpsc::Sender;

use zbus::blocking::Connection;

use crate::{
    bus_item::{BusItem, BusItems},
    config::Config,
    discovery::DiscoveredMiniserver,
    probe::TankKind,
};

pub const SERVICE_NAME: &str = "com.victronenergy.loxonetanks";
pub const DISCOVERY_LIMIT: usize = 4;

#[derive(Debug)]
pub enum Command {
    ScanMiniservers,
    SelectMiniserver(usize),
    SetHost(String),
    SetUsername(String),
    SetPassword(String),
    SetTankCapacity(TankKind, f64),
    SaveServer,
    Reconnect,
    RuntimeConnected(u64),
    RuntimeValues(u64, Vec<(String, f64)>),
    RuntimeCredentials(u64, crate::config::Credentials),
    RuntimeDisconnected(u64, String),
}

pub struct Publisher {
    connection: Connection,
    items: BusItems,
}

impl Publisher {
    pub fn new(connection: Connection, commands: Sender<Command>) -> zbus::Result<Self> {
        let mut publisher = Self {
            connection,
            items: BusItems::default(),
        };
        publisher
            .connection
            .object_server()
            .at("/", publisher.items.root())?;
        for (path, value) in [
            ("/Mgmt/ProcessName", "venus-loxone-tanks"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "Loxone Miniserver"),
            ("/ProductName", "Loxone Tanks"),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/Connection/State", "not-configured"),
            ("/Connection/StatusText", "Not configured"),
            ("/Discovery/State", "idle"),
        ] {
            publisher.add(path, BusItem::string(value))?;
        }
        for (path, value) in [
            ("/DeviceInstance", 0),
            ("/ProductId", 0),
            ("/Connected", 1),
            ("/Discovery/Count", 0),
        ] {
            publisher.add(path, BusItem::i32(value))?;
        }

        let sender = commands.clone();
        publisher.add(
            "/Config/Host",
            BusItem::writable_string("", move |value| {
                sender.send(Command::SetHost(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Config/Username",
            BusItem::writable_string("", move |value| {
                sender.send(Command::SetUsername(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Connection/Retry",
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender.send(Command::Reconnect).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Config/Password",
            BusItem::writable_string("", move |value| {
                sender.send(Command::SetPassword(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Config/SaveServer",
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender.send(Command::SaveServer).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Discovery/Scan",
            BusItem::writable_i32(0, move |value| {
                if value != 1 {
                    return 2;
                }
                sender.send(Command::ScanMiniservers).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands;
        publisher.add(
            "/Discovery/Select",
            BusItem::writable_i32(-1, move |value| {
                let Ok(index) = usize::try_from(value) else {
                    return 2;
                };
                if index >= DISCOVERY_LIMIT {
                    return 2;
                }
                sender
                    .send(Command::SelectMiniserver(index))
                    .map_or(2, |_| 0)
            }),
        )?;

        for index in 0..DISCOVERY_LIMIT {
            let root = format!("/Discovery/Results/{index}");
            for suffix in ["Address", "Serial", "Version"] {
                publisher.add(&format!("{root}/{suffix}"), BusItem::string(""))?;
            }
        }

        Ok(publisher)
    }

    pub fn register(&self) -> zbus::Result<()> {
        self.connection.request_name(SERVICE_NAME)?;
        Ok(())
    }

    pub fn publish_config(&self, config: &Config) -> zbus::Result<()> {
        self.string("/Config/Host", &config.miniserver.host)?;
        self.string("/Config/Username", &config.miniserver.username)
    }

    pub fn set_connection(&self, state: &str, text: &str) -> zbus::Result<()> {
        self.string("/Connection/State", state)?;
        self.string("/Connection/StatusText", text)
    }

    pub fn set_discovery_state(&self, state: &str) -> zbus::Result<()> {
        self.string("/Discovery/State", state)?;
        self.i32("/Discovery/Scan", 0)
    }

    pub fn publish_discovery(&self, results: &[DiscoveredMiniserver]) -> zbus::Result<()> {
        self.i32("/Discovery/Count", results.len() as i32)?;
        for index in 0..DISCOVERY_LIMIT {
            let root = format!("/Discovery/Results/{index}");
            let result = results.get(index);
            self.string(
                &format!("{root}/Address"),
                result.map(|item| item.address.as_str()).unwrap_or(""),
            )?;
            self.string(
                &format!("{root}/Serial"),
                result.map(|item| item.serial.as_str()).unwrap_or(""),
            )?;
            self.string(
                &format!("{root}/Version"),
                result.map(|item| item.version.as_str()).unwrap_or(""),
            )?;
        }
        Ok(())
    }

    pub fn set_host(&self, host: &str) -> zbus::Result<()> {
        self.string("/Config/Host", host)
    }

    pub fn set_username(&self, username: &str) -> zbus::Result<()> {
        self.string("/Config/Username", username)
    }

    pub fn set_runtime_connected(&self) -> zbus::Result<()> {
        self.string("/Connection/State", "connected")?;
        self.string("/Connection/StatusText", "Connected")?;
        self.i32("/Connection/Retry", 0)
    }

    pub fn set_runtime_disconnected(&self, text: &str) -> zbus::Result<()> {
        self.string("/Connection/State", "disconnected")?;
        self.string("/Connection/StatusText", text)?;
        self.i32("/Connection/Retry", 0)
    }

    fn add(&mut self, path: &str, item: BusItem) -> zbus::Result<()> {
        let handle = item.handle();
        self.connection.object_server().at(path, item)?;
        self.items.insert(path, handle);
        Ok(())
    }

    fn string(&self, path: &str, value: &str) -> zbus::Result<()> {
        self.items
            .handle(path)
            .set_string(&self.connection, path, value.to_owned())
    }

    fn i32(&self, path: &str, value: i32) -> zbus::Result<()> {
        self.items
            .handle(path)
            .set_i32(&self.connection, path, value)
    }
}
