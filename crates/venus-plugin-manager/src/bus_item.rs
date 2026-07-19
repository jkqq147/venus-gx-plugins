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

type WriteCallback = Arc<dyn Fn(i32) -> i32 + Send + Sync>;

pub struct BusItem {
    state: Arc<Mutex<BusItemState>>,
    write_callback: Option<WriteCallback>,
    records_write: bool,
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
            records_write: false,
        }
    }

    pub fn i32(value: i32) -> Self {
        Self {
            state: Arc::new(Mutex::new(BusItemState {
                text: value.to_string(),
                value: OwnedValue::from(value),
            })),
            write_callback: None,
            records_write: false,
        }
    }

    pub fn writable_i32(value: i32, callback: impl Fn(i32) -> i32 + Send + Sync + 'static) -> Self {
        let mut item = Self::i32(value);
        item.write_callback = Some(Arc::new(callback));
        item
    }

    pub fn trigger(callback: impl Fn() -> i32 + Send + Sync + 'static) -> Self {
        let mut item = Self::writable_i32(0, move |value| if value == 1 { callback() } else { 2 });
        item.records_write = true;
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
        let Some(callback) = self.write_callback.as_ref() else {
            return 1;
        };
        let Ok(value) = i32::try_from(value) else {
            return 2;
        };
        if !self.records_write {
            return callback(value);
        }

        // Record the trigger before dispatching it. Command handlers publish their
        // idle value after dispatch; without this transition, that reset is suppressed and
        // VBusItem can keep the command at 1, preventing a second identical click.
        let previous = {
            let mut state = self.state.lock().expect("BusItem state poisoned");
            let previous = (state.value.clone(), state.text.clone());
            state.value = OwnedValue::from(value);
            state.text = value.to_string();
            previous
        };

        let result = callback(value);
        if result != 0 {
            let mut state = self.state.lock().expect("BusItem state poisoned");
            state.value = previous.0;
            state.text = previous.1;
        }
        result
    }

    #[zbus(signal)]
    async fn properties_changed(
        emitter: &SignalEmitter<'_>,
        changes: HashMap<&str, OwnedValue>,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_updates_state_before_callback() {
        let state: Arc<Mutex<Option<Arc<Mutex<BusItemState>>>>> = Arc::new(Mutex::new(None));
        let callback_state = Arc::clone(&state);
        let item = BusItem::trigger(move || {
            let item_state = callback_state
                .lock()
                .expect("test state poisoned")
                .as_ref()
                .cloned()
                .expect("item state missing");
            let state = item_state.lock().expect("BusItem state poisoned");
            assert_eq!(i32::try_from(state.value.clone()).unwrap(), 1);
            assert_eq!(state.text, "1");
            0
        });
        *state.lock().expect("test state poisoned") = Some(Arc::clone(&item.state));

        assert_eq!(item.set_value(OwnedValue::from(1_i32)), 0);
        assert_eq!(
            item.handle().snapshot()["Text"],
            OwnedValue::from(Str::from("1"))
        );
    }

    #[test]
    fn rejected_trigger_restores_previous_state() {
        let item = BusItem::trigger(|| 2);

        assert_eq!(item.set_value(OwnedValue::from(1_i32)), 2);
        assert_eq!(
            item.handle().snapshot()["Text"],
            OwnedValue::from(Str::from("0"))
        );
    }

    #[test]
    fn ordinary_writable_item_waits_for_published_state() {
        let item = BusItem::writable_i32(0, |value| {
            assert_eq!(value, 1);
            0
        });

        assert_eq!(item.set_value(OwnedValue::from(1_i32)), 0);
        assert_eq!(
            item.handle().snapshot()["Text"],
            OwnedValue::from(Str::from("0"))
        );
    }

    #[test]
    fn trigger_rejects_non_integer_values() {
        let item = BusItem::trigger(|| 0);

        assert_eq!(item.set_value(OwnedValue::from(Str::from("1"))), 2);
        assert_eq!(
            item.handle().snapshot()["Text"],
            OwnedValue::from(Str::from("0"))
        );
    }
}
