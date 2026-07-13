use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use zbus::{
    blocking::Connection,
    interface,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Str},
};

type WriteCallback = Arc<dyn Fn(OwnedValue) -> i32 + Send + Sync>;

pub struct BusItem {
    state: Arc<Mutex<BusItemState>>,
    write_callback: Option<WriteCallback>,
}

struct BusItemState {
    value: OwnedValue,
    text: String,
}

#[derive(Clone)]
pub struct BusItemHandle {
    state: Arc<Mutex<BusItemState>>,
}

impl BusItem {
    pub fn string(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            state: Arc::new(Mutex::new(BusItemState {
                text: value.clone(),
                value: OwnedValue::from(Str::from(value)),
            })),
            write_callback: None,
        }
    }

    pub fn i32(value: i32) -> Self {
        Self {
            state: Arc::new(Mutex::new(BusItemState {
                text: value.to_string(),
                value: OwnedValue::from(value),
            })),
            write_callback: None,
        }
    }

    pub fn writable_i32(value: i32, callback: impl Fn(i32) -> i32 + Send + Sync + 'static) -> Self {
        let mut item = Self::i32(value);
        item.write_callback = Some(Arc::new(move |value| match i32::try_from(value) {
            Ok(value) => callback(value),
            Err(_) => 2,
        }));
        item
    }

    pub fn handle(&self) -> BusItemHandle {
        BusItemHandle {
            state: Arc::clone(&self.state),
        }
    }
}

impl BusItemHandle {
    pub fn snapshot(&self) -> HashMap<String, OwnedValue> {
        let state = self.state.lock().expect("BusItem state poisoned");
        HashMap::from([
            ("Value".into(), state.value.clone()),
            (
                "Text".into(),
                OwnedValue::from(Str::from(state.text.clone())),
            ),
        ])
    }

    pub fn set_i32(&self, connection: &Connection, path: &str, value: i32) -> zbus::Result<()> {
        self.set(connection, path, OwnedValue::from(value), value.to_string())
    }

    pub fn set_string(
        &self,
        connection: &Connection,
        path: &str,
        value: impl Into<String>,
    ) -> zbus::Result<()> {
        let value = value.into();
        self.set(
            connection,
            path,
            OwnedValue::from(Str::from(value.clone())),
            value,
        )
    }

    fn set(
        &self,
        connection: &Connection,
        path: &str,
        value: OwnedValue,
        text: String,
    ) -> zbus::Result<()> {
        let mut state = self.state.lock().expect("BusItem state poisoned");
        if state.text == text {
            return Ok(());
        }
        state.value = value.clone();
        state.text = text.clone();
        drop(state);

        let mut changes = HashMap::new();
        changes.insert("Value", value);
        changes.insert("Text", OwnedValue::from(Str::from(text)));
        connection.emit_signal(
            None::<()>,
            path,
            "com.victronenergy.BusItem",
            "PropertiesChanged",
            &changes,
        )
    }
}

#[interface(name = "com.victronenergy.BusItem")]
impl BusItem {
    #[zbus(name = "GetValue")]
    fn get_value(&self) -> OwnedValue {
        self.state
            .lock()
            .expect("BusItem state poisoned")
            .value
            .clone()
    }

    #[zbus(name = "GetText")]
    fn get_text(&self) -> String {
        self.state
            .lock()
            .expect("BusItem state poisoned")
            .text
            .clone()
    }

    #[zbus(name = "GetDescription")]
    fn get_description(&self, _language: String, _length: i32) -> String {
        "No description given".to_owned()
    }

    #[zbus(name = "SetValue")]
    fn set_value(&self, value: OwnedValue) -> i32 {
        self.write_callback
            .as_ref()
            .map_or(1, |callback| callback(value))
    }

    #[zbus(signal)]
    async fn properties_changed(
        emitter: &SignalEmitter<'_>,
        changes: HashMap<&str, OwnedValue>,
    ) -> zbus::Result<()>;
}
