use crate::state::{Reading, Wheel};

pub enum Event {
    Reading(Reading),
    ScanStats {
        device_count: usize,
        manufacturer_data_count: usize,
    },
    Assign {
        index: usize,
        wheel: Option<Wheel>,
    },
    ScannerFailed(String),
}
