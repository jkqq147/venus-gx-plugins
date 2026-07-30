use crate::state::{Reading, Wheel};

pub enum Event {
    Reading(Box<Reading>),
    BluetoothStats {
        device_count: usize,
        manufacturer_data_count: usize,
    },
    Rtl433Frame {
        generation: u64,
    },
    Rtl433Report {
        generation: u64,
        fsk_frames: u32,
        decoder_events: u32,
        decoded_ok: u32,
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
