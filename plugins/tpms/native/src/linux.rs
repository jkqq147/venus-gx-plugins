use std::collections::HashMap;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use serde::Serialize;
use tpms_ble_parser::tpms::{parse_manufacturer_data, TpmsReading};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_HCI: libc::c_int = 1;
const SOL_HCI: libc::c_int = 0;
const HCI_FILTER: libc::c_int = 2;
const HCI_EVENT_PKT: u8 = 0x04;
const EVT_LE_META_EVENT: u8 = 0x3e;
const EVT_LE_ADVERTISING_REPORT: u8 = 0x02;
const IDENTICAL_READING_INTERVAL_SECONDS: u64 = 1;
const RECENT_DEVICE_SECONDS: u64 = 60;
const STATS_INTERVAL_SECONDS: u64 = 10;

#[repr(C)]
struct SockaddrHci {
    hci_family: libc::sa_family_t,
    hci_dev: u16,
    hci_channel: u16,
}

#[derive(Debug, Serialize)]
pub struct Observation {
    pub peripheral_address: String,
    pub local_name: Option<String>,
    pub manufacturer_data_hex: String,
    pub rssi: i8,
    pub reading: TpmsReading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    pub device_count: usize,
    pub manufacturer_data_count: usize,
}

#[derive(Clone, PartialEq)]
struct ReadingSignature {
    pressure_bits: u32,
    temperature_bits: u32,
    battery_percent: u8,
    alarm: u8,
}

impl From<&TpmsReading> for ReadingSignature {
    fn from(reading: &TpmsReading) -> Self {
        Self {
            pressure_bits: reading.pressure_bar.to_bits(),
            temperature_bits: reading.temperature_c.to_bits(),
            battery_percent: reading.battery_percent,
            alarm: reading.alarm,
        }
    }
}

struct LastEmission {
    signature: ReadingSignature,
    at: Instant,
}

#[allow(dead_code)]
pub fn run() -> io::Result<()> {
    run_with_handler(|observation| {
        println!(
            "{}",
            serde_json::to_string(&observation).expect("TPMS observation must serialize")
        );
    })
}

pub fn run_with_handler<F>(handler: F) -> io::Result<()>
where
    F: FnMut(Observation),
{
    let adapter_index = adapter_index_from_args()?;
    run_with_handlers(adapter_index, handler, |_| {})
}

pub fn run_with_handlers<F, S>(
    adapter_index: u16,
    mut handler: F,
    mut stats_handler: S,
) -> io::Result<()>
where
    F: FnMut(Observation),
    S: FnMut(ScanStats),
{
    let socket = open_hci_socket(adapter_index)?;
    let mut last_emission = HashMap::<String, LastEmission>::new();
    let mut names = HashMap::<String, String>::new();
    let mut recent_devices = HashMap::<String, Instant>::new();
    let mut recent_manufacturer_devices = HashMap::<String, Instant>::new();
    let mut last_stats = Instant::now() - Duration::from_secs(STATS_INTERVAL_SECONDS);
    let mut packet = [0_u8; 2048];

    eprintln!("monitoring raw HCI advertisements on hci{adapter_index}; press Ctrl-C to stop");
    loop {
        let read = unsafe {
            libc::read(
                socket.as_raw_fd(),
                packet.as_mut_ptr().cast::<libc::c_void>(),
                packet.len(),
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            continue;
        }

        for report in parse_advertising_reports(&packet[..read as usize]) {
            let now = Instant::now();
            recent_devices.insert(report.address.clone(), now);
            if let Some(name) = local_name(report.data) {
                names.insert(report.address.clone(), name);
            }
            for manufacturer_data in manufacturer_data_fields(report.data) {
                recent_manufacturer_devices.insert(report.address.clone(), now);
                let Ok(reading) = parse_manufacturer_data(manufacturer_data) else {
                    continue;
                };
                let sensor_id = reading.sensor_id_hex.clone();
                let signature = ReadingSignature::from(&reading);
                let now = Instant::now();
                if let Some(previous) = last_emission.get(&sensor_id) {
                    if previous.signature == signature
                        && now.duration_since(previous.at).as_secs()
                            < IDENTICAL_READING_INTERVAL_SECONDS
                    {
                        continue;
                    }
                }
                last_emission.insert(sensor_id, LastEmission { signature, at: now });
                let observation = Observation {
                    peripheral_address: report.address.clone(),
                    local_name: names.get(&report.address).cloned(),
                    manufacturer_data_hex: hex_upper(manufacturer_data),
                    rssi: report.rssi,
                    reading,
                };
                handler(observation);
            }

            if last_stats.elapsed() >= Duration::from_secs(STATS_INTERVAL_SECONDS) {
                let cutoff = Duration::from_secs(RECENT_DEVICE_SECONDS);
                recent_devices.retain(|_, seen| seen.elapsed() <= cutoff);
                recent_manufacturer_devices.retain(|_, seen| seen.elapsed() <= cutoff);
                names.retain(|address, _| recent_devices.contains_key(address));
                last_emission.retain(|_, emission| emission.at.elapsed() <= cutoff);
                stats_handler(ScanStats {
                    device_count: recent_devices.len(),
                    manufacturer_data_count: recent_manufacturer_devices.len(),
                });
                last_stats = Instant::now();
            }
        }
    }
}

fn adapter_index_from_args() -> io::Result<u16> {
    let mut args = std::env::args().skip(1);
    match args.next() {
        None => Ok(0),
        Some(value) => value.parse::<u16>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: venus-tpms-hci-monitor [adapter-index]",
            )
        }),
    }
}

fn open_hci_socket(adapter_index: u16) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(AF_BLUETOOTH, libc::SOCK_RAW, BTPROTO_HCI) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let address = SockaddrHci {
        hci_family: AF_BLUETOOTH as libc::sa_family_t,
        hci_dev: adapter_index,
        hci_channel: 0,
    };
    let bind_result = unsafe {
        libc::bind(
            fd,
            (&address as *const SockaddrHci).cast::<libc::sockaddr>(),
            size_of::<SockaddrHci>() as libc::socklen_t,
        )
    };
    if bind_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }

    let event_mask = 1_u32 << (EVT_LE_META_EVENT - 32);
    let mut filter = Vec::with_capacity(14);
    filter.extend_from_slice(&(1_u32 << HCI_EVENT_PKT).to_ne_bytes());
    filter.extend_from_slice(&0_u32.to_ne_bytes());
    filter.extend_from_slice(&event_mask.to_ne_bytes());
    filter.extend_from_slice(&0_u16.to_ne_bytes());
    let filter_result = unsafe {
        libc::setsockopt(
            fd,
            SOL_HCI,
            HCI_FILTER,
            filter.as_ptr().cast::<libc::c_void>(),
            filter.len() as libc::socklen_t,
        )
    };
    if filter_result != 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }

    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

struct AdvertisingReport<'a> {
    address: String,
    data: &'a [u8],
    rssi: i8,
}

fn parse_advertising_reports(packet: &[u8]) -> Vec<AdvertisingReport<'_>> {
    let packet = packet.strip_prefix(&[HCI_EVENT_PKT]).unwrap_or(packet);
    if packet.len() < 4 || packet[0] != EVT_LE_META_EVENT {
        return Vec::new();
    }
    let parameters = &packet[2..packet.len().min(2 + packet[1] as usize)];
    if parameters.len() < 2 || parameters[0] != EVT_LE_ADVERTISING_REPORT {
        return Vec::new();
    }

    let mut reports = Vec::with_capacity(parameters[1] as usize);
    let mut offset = 2;
    for _ in 0..parameters[1] {
        if offset + 9 > parameters.len() {
            break;
        }
        let address_bytes = &parameters[offset + 2..offset + 8];
        let data_length = parameters[offset + 8] as usize;
        let data_start = offset + 9;
        let data_end = data_start + data_length;
        if data_end >= parameters.len() {
            break;
        }
        reports.push(AdvertisingReport {
            address: address_bytes
                .iter()
                .rev()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(":"),
            data: &parameters[data_start..data_end],
            rssi: parameters[data_end] as i8,
        });
        offset = data_end + 1;
    }
    reports
}

fn manufacturer_data_fields(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let length = data[offset] as usize;
        offset += 1;
        if length == 0 || offset + length > data.len() {
            break;
        }
        if data[offset] == 0xff {
            fields.push(&data[offset + 1..offset + length]);
        }
        offset += length;
    }
    fields.into_iter()
}

fn local_name(data: &[u8]) -> Option<String> {
    let mut offset = 0;
    while offset < data.len() {
        let length = data[offset] as usize;
        offset += 1;
        if length == 0 || offset + length > data.len() {
            break;
        }
        if matches!(data[offset], 0x08 | 0x09) {
            return Some(String::from_utf8_lossy(&data[offset + 1..offset + length]).into_owned());
        }
        offset += length;
    }
    None
}

fn hex_upper(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{manufacturer_data_fields, parse_advertising_reports};

    #[test]
    fn extracts_tpms_manufacturer_payload_from_advertising_report() {
        let packet = [
            0x04, 0x3e, 0x20, 0x02, 0x01, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 20, 19, 0xff, 0x00, 0x01,
            0x82, 0xea, 0xca, 0x32, 0x5e, 0xeb, 0x15, 0x6b, 0x09, 0x00, 0x79, 0x0d, 0x00, 0x00,
            0x64, 0x00, 0xb0,
        ];
        let reports = parse_advertising_reports(&packet);
        assert_eq!(reports.len(), 1);
        let fields = manufacturer_data_fields(reports[0].data).collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                &[
                    0x00, 0x01, 0x82, 0xea, 0xca, 0x32, 0x5e, 0xeb, 0x15, 0x6b, 0x09, 0x00, 0x79,
                    0x0d, 0x00, 0x00, 0x64, 0x00
                ][..]
            ]
        );
    }
}
