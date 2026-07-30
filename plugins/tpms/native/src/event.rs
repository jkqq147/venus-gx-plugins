use crate::state::{Reading, Wheel};

pub enum Event {
    Reading(Reading),
    BluetoothStats {
        device_count: usize,
        manufacturer_data_count: usize,
    },
    Rtl433Frame {
        generation: u64,
    },
    Rtl433Exited {
        generation: u64,
        success: bool,
    },
    Assign {
        index: usize,
        wheel: Option<Wheel>,
    },
    ScannerFailed(String),
}
