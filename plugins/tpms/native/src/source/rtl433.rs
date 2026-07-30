use serde::Deserialize;

use crate::state::{unix_time, Reading, SourceKind};

pub const PROTOCOL_NUMBER: u32 = 59;
pub const RETRY_MIN_SECONDS: u64 = 5;
pub const RETRY_MAX_SECONDS: u64 = 60;
#[cfg(target_os = "linux")]
pub const STATS_INTERVAL_SECONDS: u64 = 30;
pub const STATS_HEARTBEAT_TIMEOUT_SECONDS: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rtl433Report {
    pub fsk_frames: u32,
    pub decoder_events: u32,
    pub decoded_ok: u32,
}

#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    record_type: Option<String>,
    model: Option<String>,
    id: Option<String>,
    #[serde(rename = "pressure_kPa")]
    pressure_kpa: Option<f64>,
    #[serde(rename = "temperature_C")]
    temperature_c: Option<f64>,
    #[serde(rename = "battery_mV")]
    battery_mv: Option<i32>,
    alarm: Option<String>,
    mic: Option<String>,
    rssi: Option<f64>,
    snr: Option<f64>,
    noise: Option<f64>,
    enabled: Option<u32>,
    frames: Option<FrameReport>,
    #[serde(default)]
    stats: Vec<DecoderReport>,
}

#[derive(Debug, Deserialize)]
struct FrameReport {
    #[serde(default)]
    fsk: u32,
}

#[derive(Debug, Deserialize)]
struct DecoderReport {
    device: Option<u32>,
    #[serde(default)]
    events: u32,
    #[serde(default)]
    ok: u32,
}

enum ParsedLine {
    Reading(Box<Reading>),
    Report(Rtl433Report),
    Ignored,
}

fn parse_output_line(line: &str) -> Result<ParsedLine, serde_json::Error> {
    let record: Record = serde_json::from_str(line)?;
    if record.enabled.is_some() {
        if let Some(frames) = record.frames {
            let steelmate = record
                .stats
                .iter()
                .filter(|decoder| decoder.device == Some(PROTOCOL_NUMBER))
                .fold((0_u32, 0_u32), |(events, ok), decoder| {
                    (
                        events.saturating_add(decoder.events),
                        ok.saturating_add(decoder.ok),
                    )
                });
            return Ok(ParsedLine::Report(Rtl433Report {
                fsk_frames: frames.fsk,
                decoder_events: steelmate.0,
                decoded_ok: steelmate.1,
            }));
        }
    }

    if record.record_type.as_deref() != Some("TPMS")
        || record.model.as_deref() != Some("Steelmate")
        || record.mic.as_deref() != Some("CHECKSUM")
    {
        return Ok(ParsedLine::Ignored);
    }

    let Some(sensor_id) = record.id.filter(|id| !id.is_empty()) else {
        return Ok(ParsedLine::Ignored);
    };
    let Some(pressure_kpa) = record
        .pressure_kpa
        .filter(|value| value.is_finite() && *value >= 0.0)
    else {
        return Ok(ParsedLine::Ignored);
    };
    let sensor_id = sensor_id.to_ascii_lowercase();
    Ok(ParsedLine::Reading(Box::new(Reading {
        sensor_key: format!("rtl433:steelmate:{sensor_id}"),
        sensor_id: sensor_id.clone(),
        source: SourceKind::Rtl433,
        protocol: format!("Steelmate {PROTOCOL_NUMBER}"),
        name: format!("Steelmate {sensor_id}"),
        pressure_bar: pressure_kpa / 100.0,
        temperature_c: record.temperature_c.filter(|value| value.is_finite()),
        battery_percent: None,
        battery_mv: record.battery_mv,
        alarm: record.alarm.filter(|value| !value.trim().is_empty()),
        signal_value: record.rssi.filter(|value| value.is_finite()),
        signal_unit: "dB".to_owned(),
        snr_db: record.snr.filter(|value| value.is_finite()),
        noise_db: record.noise.filter(|value| value.is_finite()),
        last_seen: unix_time(),
        manufacturer_data_hex: None,
    })))
}

#[cfg(test)]
pub fn parse_line(line: &str) -> Result<Option<Reading>, serde_json::Error> {
    Ok(match parse_output_line(line)? {
        ParsedLine::Reading(reading) => Some(*reading),
        ParsedLine::Report(_) | ParsedLine::Ignored => None,
    })
}

#[cfg(test)]
pub fn parse_report_line(line: &str) -> Result<Option<Rtl433Report>, serde_json::Error> {
    Ok(match parse_output_line(line)? {
        ParsedLine::Report(report) => Some(report),
        ParsedLine::Reading(_) | ParsedLine::Ignored => None,
    })
}

pub fn retry_delay_seconds(failures: u32) -> u64 {
    let exponent = failures.saturating_sub(1).min(6);
    (RETRY_MIN_SECONDS << exponent).min(RETRY_MAX_SECONDS)
}

#[cfg(target_os = "linux")]
mod process {
    use std::{
        io::{self, BufRead, BufReader},
        os::unix::process::CommandExt,
        path::Path,
        process::{Command, Stdio},
        sync::{mpsc::Sender, Arc, Condvar, Mutex},
        thread,
        time::Duration,
    };

    use crate::event::Event;

    use super::{parse_output_line, ParsedLine, PROTOCOL_NUMBER, STATS_INTERVAL_SECONDS};

    const TERMINATE_GRACE: Duration = Duration::from_secs(2);
    const KILL_GRACE: Duration = Duration::from_secs(2);
    pub struct Rtl433Process {
        pid: u32,
        running: bool,
        exited: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Rtl433Process {
        pub fn start(binary: &Path, generation: u64, events: Sender<Event>) -> io::Result<Self> {
            let mut command = Command::new(binary);
            command
                .args([
                    "-d", "0", "-f", "433.92M", "-s", "250k", "-b", "32768", "-R",
                ])
                .arg(PROTOCOL_NUMBER.to_string())
                .args(["-F", "json", "-M", "time", "-M", "level", "-M"])
                .arg(format!("stats:3:{STATS_INTERVAL_SECONDS}"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let parent = std::process::id();
            // SAFETY: pre_exec calls only async-signal-safe libc functions.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "TPMS adapter exited while starting rtl_433",
                        ));
                    }
                    Ok(())
                });
            }

            let mut child = command.spawn()?;
            let pid = child.id();
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("rtl_433 stdout was not captured"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("rtl_433 stderr was not captured"))?;

            let stdout_events = events.clone();
            if let Err(error) =
                thread::Builder::new()
                    .name("rtl433-json".to_owned())
                    .spawn(move || {
                        for line in BufReader::new(stdout).lines() {
                            let Ok(line) = line else {
                                break;
                            };
                            match parse_output_line(&line) {
                                Ok(ParsedLine::Reading(reading)) => {
                                    let _ = stdout_events.send(Event::Rtl433Reading {
                                        generation,
                                        reading,
                                    });
                                }
                                Ok(ParsedLine::Report(report)) => {
                                    let _ = stdout_events.send(Event::Rtl433Report {
                                        generation,
                                        fsk_frames: report.fsk_frames,
                                        decoder_events: report.decoder_events,
                                        decoded_ok: report.decoded_ok,
                                    });
                                }
                                Ok(ParsedLine::Ignored) => {}
                                Err(error) => eprintln!("invalid rtl_433 JSON: {error}"),
                            }
                        }
                    })
            {
                kill_and_reap(pid);
                return Err(error);
            }

            if let Err(error) =
                thread::Builder::new()
                    .name("rtl433-log".to_owned())
                    .spawn(move || {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            eprintln!("rtl_433: {line}");
                        }
                    })
            {
                kill_and_reap(pid);
                return Err(error);
            }

            let wait_events = events.clone();
            let exited = Arc::new((Mutex::new(false), Condvar::new()));
            let wait_exited = Arc::clone(&exited);
            if let Err(error) =
                thread::Builder::new()
                    .name("rtl433-wait".to_owned())
                    .spawn(move || {
                        let mut status = 0;
                        let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
                        let success = waited == pid as libc::pid_t
                            && libc::WIFEXITED(status)
                            && libc::WEXITSTATUS(status) == 0;
                        let (lock, condition) = &*wait_exited;
                        if let Ok(mut exited) = lock.lock() {
                            *exited = true;
                            condition.notify_all();
                        }
                        let _ = wait_events.send(Event::Rtl433Exited {
                            generation,
                            success,
                        });
                    })
            {
                kill_and_reap(pid);
                return Err(error);
            }

            drop(child);
            Ok(Self {
                pid,
                running: true,
                exited,
            })
        }

        pub fn mark_exited(&mut self) {
            self.running = false;
        }

        pub fn stop(&mut self) -> io::Result<()> {
            if !self.running {
                return Ok(());
            }
            send_signal(self.pid, libc::SIGTERM)?;
            if wait_until_exited(&self.exited, TERMINATE_GRACE)? {
                self.running = false;
                return Ok(());
            }

            send_signal(self.pid, libc::SIGKILL)?;
            if !wait_until_exited(&self.exited, KILL_GRACE)? {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("rtl_433 process {} did not stop", self.pid),
                ));
            }
            self.running = false;
            Ok(())
        }
    }

    impl Drop for Rtl433Process {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    fn send_signal(pid: u32, signal: libc::c_int) -> io::Result<()> {
        let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn kill_and_reap(pid: u32) {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
            let mut status = 0;
            libc::waitpid(pid as libc::pid_t, &mut status, 0);
        }
    }

    fn wait_until_exited(
        exited: &Arc<(Mutex<bool>, Condvar)>,
        timeout: Duration,
    ) -> io::Result<bool> {
        let (lock, condition) = &**exited;
        let exited = lock
            .lock()
            .map_err(|_| io::Error::other("rtl_433 exit state poisoned"))?;
        let (exited, _) = condition
            .wait_timeout_while(exited, timeout, |exited| !*exited)
            .map_err(|_| io::Error::other("rtl_433 exit state poisoned"))?;
        Ok(*exited)
    }
}

#[cfg(target_os = "linux")]
pub use process::Rtl433Process;

#[cfg(test)]
mod tests {
    use super::{parse_line, parse_report_line, retry_delay_seconds, Rtl433Report};
    use crate::state::SourceKind;

    #[test]
    fn parses_high_pressure_steelmate_frame() {
        let reading = parse_line(
            r#"{"type":"TPMS","model":"Steelmate","id":"0x7721","pressure_kPa":631.250,"temperature_C":35,"battery_mV":2970,"mic":"CHECKSUM","rssi":-6.741,"snr":17.845,"noise":-24.585}"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(reading.sensor_key, "rtl433:steelmate:0x7721");
        assert_eq!(reading.source, SourceKind::Rtl433);
        assert_eq!(reading.pressure_bar, 6.3125);
        assert_eq!(reading.battery_mv, Some(2970));
        assert_eq!(reading.signal_unit, "dB");
    }

    #[test]
    fn keeps_valid_fffd_sensor_and_fast_leak_alarm() {
        let reading = parse_line(
            r#"{"type":"TPMS","model":"Steelmate","id":"0xfffd","pressure_kPa":0.0,"temperature_C":36,"alarm":"fast leak","mic":"CHECKSUM"}"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(reading.sensor_id, "0xfffd");
        assert_eq!(reading.alarm.as_deref(), Some("fast leak"));
    }

    #[test]
    fn parses_old_single_steelmate_sensor() {
        let reading = parse_line(
            r#"{"type":"TPMS","model":"Steelmate","id":"0xc4f2","pressure_kPa":137.895,"temperature_C":24,"battery_mV":3000,"mic":"CHECKSUM"}"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(reading.sensor_key, "rtl433:steelmate:0xc4f2");
        assert_eq!(reading.sensor_id, "0xc4f2");
        assert!((reading.pressure_bar - 1.37895).abs() < 1e-9);
    }

    #[test]
    fn rejects_toyota_and_bad_checksum_records() {
        assert!(parse_line(
            r#"{"type":"TPMS","model":"Toyota","id":"dc5081e1","pressure_PSI":35.0,"mic":"CRC"}"#,
        )
        .unwrap()
        .is_none());
        assert!(parse_line(
            r#"{"type":"TPMS","model":"Steelmate","id":"0xc4f2","pressure_kPa":350.0,"mic":"BAD"}"#,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn parses_live_rtl433_stats_report() {
        let report = parse_report_line(
            r#"{"time":"2026-07-30 14:22:18","enabled":1,"since":"2026-07-30 14:21:48","frames":{"count":2,"fsk":2,"events":0},"stats":[{"device":59,"name":"Steelmate TPMS","events":2,"ok":0,"messages":0,"fail_sanity":2}]}"#,
        )
        .unwrap();

        assert_eq!(
            report,
            Some(Rtl433Report {
                fsk_frames: 2,
                decoder_events: 2,
                decoded_ok: 0,
            })
        );
    }

    #[test]
    fn report_parser_ignores_regular_and_unrelated_json() {
        assert!(parse_report_line(
            r#"{"type":"TPMS","model":"Toyota","id":"dc5081e1","pressure_PSI":35.0,"mic":"CRC"}"#,
        )
        .unwrap()
        .is_none());
        assert!(parse_report_line(r#"{"enabled":1,"stats":[]}"#)
            .unwrap()
            .is_none());
    }

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay_seconds(1), 5);
        assert_eq!(retry_delay_seconds(2), 10);
        assert_eq!(retry_delay_seconds(5), 60);
        assert_eq!(retry_delay_seconds(u32::MAX), 60);
    }
}
