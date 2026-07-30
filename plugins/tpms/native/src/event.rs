use crate::state::{Reading, ReceiverMode, Wheel};

pub enum Event {
    BleReading {
        generation: u64,
        reading: Box<Reading>,
    },
    BluetoothStats {
        generation: u64,
        device_count: usize,
        manufacturer_data_count: usize,
    },
    Rtl433Reading {
        generation: u64,
        reading: Box<Reading>,
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
    SetReceiverMode(ReceiverMode),
    BleScannerFailed {
        generation: u64,
        error: String,
    },
}
