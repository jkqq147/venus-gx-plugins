use std::sync::mpsc::Sender;

use zbus::blocking::Connection;

use crate::{
    bus_item::{BusItem, BusItems},
    config::{LoadMode, ManagedConfig, MAX_SERVICES},
};

pub const SERVICE_NAME: &str = "com.victronenergy.rathole";

#[derive(Debug, Clone)]
pub enum Command {
    SetServerHost(String),
    SetServerPort(String),
    SetDeviceName(String),
    Save,
    ConfirmRename,
    SetServiceSlug(usize, String),
    SetServiceHost(usize, String),
    SetServicePort(usize, String),
    DeleteService(usize),
    SetAddPreset(String),
    SetAddSlug(String),
    SetAddHost(String),
    SetAddPort(String),
    AddService,
    ChildExited { generation: u64, success: bool },
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
            ("/Mgmt/ProcessName", "venus-rathole"),
            ("/Mgmt/ProcessVersion", env!("CARGO_PKG_VERSION")),
            ("/Mgmt/Connection", "Rathole client"),
            ("/ProductName", "Rathole"),
            ("/FirmwareVersion", env!("CARGO_PKG_VERSION")),
            ("/Status/State", "starting"),
            ("/Status/Text", "Starting"),
            ("/Config/Mode", "missing"),
            ("/Config/Detail", "not-configured"),
            ("/Config/Feedback", ""),
            ("/Config/FeedbackText", ""),
            ("/Config/Token", ""),
            ("/Config/OriginalDeviceName", ""),
        ] {
            publisher.add(path, BusItem::string(value))?;
        }
        for (path, value) in [
            ("/DeviceInstance", 0),
            ("/ProductId", 0),
            ("/Connected", 1),
            ("/Config/Dirty", 0),
            ("/Config/RenameConfirmation", 0),
            ("/Services/Count", 0),
        ] {
            publisher.add(path, BusItem::i32(value))?;
        }

        let sender = commands.clone();
        publisher.add(
            "/Config/Host",
            BusItem::writable_string("", move |value| {
                sender.send(Command::SetServerHost(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Config/Port",
            BusItem::writable_string("2333", move |value| {
                sender.send(Command::SetServerPort(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Config/DeviceName",
            BusItem::writable_string("", move |value| {
                sender.send(Command::SetDeviceName(value)).map_or(2, |_| 0)
            }),
        )?;
        add_command(
            &mut publisher,
            "/Config/Save",
            commands.clone(),
            Command::Save,
        )?;
        add_command(
            &mut publisher,
            "/Config/ConfirmRename",
            commands.clone(),
            Command::ConfirmRename,
        )?;

        for index in 0..MAX_SERVICES {
            let root = format!("/Services/{index}");
            publisher.add(&format!("{root}/Visible"), BusItem::i32(0))?;
            publisher.add(&format!("{root}/Name"), BusItem::string(""))?;
            publisher.add(&format!("{root}/Summary"), BusItem::string(""))?;

            let sender = commands.clone();
            publisher.add(
                &format!("{root}/Slug"),
                BusItem::writable_string("", move |value| {
                    sender
                        .send(Command::SetServiceSlug(index, value))
                        .map_or(2, |_| 0)
                }),
            )?;
            let sender = commands.clone();
            publisher.add(
                &format!("{root}/Host"),
                BusItem::writable_string("", move |value| {
                    sender
                        .send(Command::SetServiceHost(index, value))
                        .map_or(2, |_| 0)
                }),
            )?;
            let sender = commands.clone();
            publisher.add(
                &format!("{root}/Port"),
                BusItem::writable_string("", move |value| {
                    sender
                        .send(Command::SetServicePort(index, value))
                        .map_or(2, |_| 0)
                }),
            )?;
            add_command(
                &mut publisher,
                &format!("{root}/Delete"),
                commands.clone(),
                Command::DeleteService(index),
            )?;
        }

        let sender = commands.clone();
        publisher.add(
            "/Services/Add/Preset",
            BusItem::writable_string("homeassistant", move |value| {
                sender.send(Command::SetAddPreset(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Services/Add/Slug",
            BusItem::writable_string("homeassistant", move |value| {
                sender.send(Command::SetAddSlug(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Services/Add/Host",
            BusItem::writable_string("127.0.0.1", move |value| {
                sender.send(Command::SetAddHost(value)).map_or(2, |_| 0)
            }),
        )?;
        let sender = commands.clone();
        publisher.add(
            "/Services/Add/Port",
            BusItem::writable_string("8123", move |value| {
                sender.send(Command::SetAddPort(value)).map_or(2, |_| 0)
            }),
        )?;
        add_command(
            &mut publisher,
            "/Services/Add/Commit",
            commands,
            Command::AddService,
        )?;
        Ok(publisher)
    }

    pub fn register(&self) -> zbus::Result<()> {
        self.connection.request_name(SERVICE_NAME)?;
        Ok(())
    }

    pub fn publish_mode(&self, mode: LoadMode, detail: &str) -> zbus::Result<()> {
        self.string(
            "/Config/Mode",
            match mode {
                LoadMode::Missing => "missing",
                LoadMode::Managed => "managed",
                LoadMode::Advanced => "advanced",
                LoadMode::Invalid => "invalid",
            },
        )?;
        self.string("/Config/Detail", detail)
    }

    pub fn publish_config(&self, config: &ManagedConfig) -> zbus::Result<()> {
        self.string("/Config/Host", &config.server_host)?;
        self.string("/Config/Port", &config.server_port.to_string())?;
        self.string("/Config/DeviceName", &config.device_name)?;
        self.string("/Config/Token", &config.token)?;
        self.publish_services(config)
    }

    pub fn publish_services(&self, config: &ManagedConfig) -> zbus::Result<()> {
        self.i32("/Services/Count", config.services.len() as i32)?;
        for index in 0..MAX_SERVICES {
            let root = format!("/Services/{index}");
            if let Some(service) = config.services.get(index) {
                self.i32(&format!("{root}/Visible"), 1)?;
                self.string(
                    &format!("{root}/Name"),
                    &service.generated_name(&config.device_name),
                )?;
                self.string(&format!("{root}/Summary"), &service.summary())?;
                self.string(&format!("{root}/Slug"), &service.slug)?;
                self.string(&format!("{root}/Host"), &service.local_host)?;
                self.string(&format!("{root}/Port"), &service.local_port.to_string())?;
            } else {
                self.i32(&format!("{root}/Visible"), 0)?;
                for suffix in ["Name", "Summary", "Slug", "Host"] {
                    self.string(&format!("{root}/{suffix}"), "")?;
                }
                self.string(&format!("{root}/Port"), "")?;
                self.i32(&format!("{root}/Delete"), 0)?;
            }
        }
        Ok(())
    }

    pub fn publish_add_editor(
        &self,
        preset: &str,
        slug: &str,
        host: &str,
        port: u16,
    ) -> zbus::Result<()> {
        self.string("/Services/Add/Preset", preset)?;
        self.string("/Services/Add/Slug", slug)?;
        self.string("/Services/Add/Host", host)?;
        self.string("/Services/Add/Port", &port.to_string())
    }

    pub fn set_original_device_name(&self, name: &str) -> zbus::Result<()> {
        self.string("/Config/OriginalDeviceName", name)
    }

    pub fn set_config_feedback(&self, state: &str, text: &str) -> zbus::Result<()> {
        self.string("/Config/Feedback", state)?;
        self.string("/Config/FeedbackText", text)
    }

    pub fn set_status(&self, state: &str, text: &str) -> zbus::Result<()> {
        self.string("/Status/State", state)?;
        self.string("/Status/Text", text)
    }

    pub fn set_dirty(&self, dirty: bool) -> zbus::Result<()> {
        self.i32("/Config/Dirty", i32::from(dirty))
    }

    pub fn set_rename_confirmation(&self, required: bool) -> zbus::Result<()> {
        self.i32("/Config/RenameConfirmation", i32::from(required))
    }

    pub fn reset_commands(&self) -> zbus::Result<()> {
        for path in [
            "/Config/Save",
            "/Config/ConfirmRename",
            "/Services/Add/Commit",
        ] {
            self.i32(path, 0)?;
        }
        for index in 0..MAX_SERVICES {
            self.i32(&format!("/Services/{index}/Delete"), 0)?;
        }
        Ok(())
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

fn add_command(
    publisher: &mut Publisher,
    path: &str,
    commands: Sender<Command>,
    command: Command,
) -> zbus::Result<()> {
    publisher.add(
        path,
        BusItem::writable_i32(0, move |value| {
            if value != 1 {
                return 2;
            }
            commands.send(command.clone()).map_or(2, |_| 0)
        }),
    )
}
