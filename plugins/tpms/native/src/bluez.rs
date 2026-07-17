use std::{collections::HashMap, fs, io};

use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Str},
};

const BLUEZ_SERVICE: &str = "org.bluez";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

pub struct BluezDiscovery<'a> {
    adapter: Proxy<'a>,
    properties: Proxy<'a>,
    owned: bool,
}

impl<'a> BluezDiscovery<'a> {
    pub fn start(connection: &'a Connection, adapter_index: u16) -> zbus::Result<Self> {
        let path = OwnedObjectPath::try_from(format!("/org/bluez/hci{adapter_index}"))?;
        let adapter = Proxy::new(connection, BLUEZ_SERVICE, path.clone(), ADAPTER_INTERFACE)?;
        let properties = Proxy::new(connection, BLUEZ_SERVICE, path, PROPERTIES_INTERFACE)?;
        let discovering: OwnedValue =
            properties.call("Get", &(ADAPTER_INTERFACE, "Discovering"))?;
        let discovering = bool::try_from(discovering).unwrap_or(false);

        if !discovering {
            let mut filter = HashMap::<&str, OwnedValue>::new();
            filter.insert("Transport", OwnedValue::from(Str::from("le")));
            filter.insert("DuplicateData", OwnedValue::from(false));
            let _: () = adapter.call("SetDiscoveryFilter", &(filter,))?;
            let _: () = adapter.call("StartDiscovery", &())?;
        }

        Ok(Self {
            adapter,
            properties,
            owned: !discovering,
        })
    }

    pub fn is_active(&self) -> bool {
        let discovering: zbus::Result<OwnedValue> = self
            .properties
            .call("Get", &(ADAPTER_INTERFACE, "Discovering"));
        discovering
            .ok()
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false)
    }
}

pub fn lowest_adapter_index() -> io::Result<u16> {
    let mut indexes = fs::read_dir("/sys/class/bluetooth")?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("hci")
                .and_then(|value| value.parse::<u16>().ok())
        })
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Bluetooth HCI adapter found"))
}

impl Drop for BluezDiscovery<'_> {
    fn drop(&mut self) {
        if self.owned {
            let _: zbus::Result<()> = self.adapter.call("StopDiscovery", &());
        }
    }
}
