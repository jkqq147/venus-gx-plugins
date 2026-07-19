use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    config::{Config, Credentials},
    loxone::{LoxoneError, Packet, Session},
    probe::require_unique_tanks,
    publisher::Command,
};

const LOXONE_EPOCH_UNIX: u64 = 1_230_768_000;
const TOKEN_REFRESH_WINDOW: u64 = 7 * 24 * 60 * 60;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4 * 60);
const STABLE_CONNECTION_WINDOW: Duration = Duration::from_secs(60);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

pub struct RuntimeHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl RuntimeHandle {
    pub fn start(
        generation: u64,
        config: Config,
        credentials: Credentials,
        commands: Sender<Command>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("loxone-events".to_owned())
            .spawn(move || run_runtime(generation, config, credentials, commands, worker_stop))
            .expect("failed to start Loxone event thread");
        Self {
            stop,
            join: Some(join),
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        self.stop();
        // The socket has a bounded read timeout. Detaching here keeps the D-Bus
        // control loop responsive; generation IDs discard any late events.
        let _ = self.join.take();
    }
}

fn run_runtime(
    generation: u64,
    config: Config,
    mut credentials: Credentials,
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
) {
    let mut retry_attempt = 0_u32;
    while !stop.load(Ordering::Acquire) {
        match run_session(
            generation,
            &config,
            &mut credentials,
            &commands,
            &stop,
            &mut retry_attempt,
        ) {
            Ok(()) => return,
            Err(error) if error.is_retryable() => {
                if commands
                    .send(Command::RuntimeReconnecting(
                        generation,
                        public_error(&error),
                    ))
                    .is_err()
                {
                    return;
                }
                let delay = retry_delay(retry_attempt);
                retry_attempt = retry_attempt.saturating_add(1);
                if !wait_for_retry(&stop, delay) {
                    return;
                }
            }
            Err(error) => {
                let _ = commands.send(Command::RuntimeDisconnected(
                    generation,
                    public_error(&error),
                ));
                return;
            }
        }
    }
}

fn run_session(
    generation: u64,
    config: &Config,
    credentials: &mut Credentials,
    commands: &Sender<Command>,
    stop: &AtomicBool,
    retry_attempt: &mut u32,
) -> Result<(), LoxoneError> {
    let mut session = Session::authenticated(
        &config.miniserver.host,
        &config.miniserver.username,
        &credentials.token,
    )?;

    if token_needs_refresh(credentials.valid_until) {
        let refreshed = session.refresh_token(&config.miniserver.username, credentials)?;
        if commands
            .send(Command::RuntimeCredentials(generation, refreshed.clone()))
            .is_err()
        {
            return Ok(());
        }
        *credentials = refreshed;
    }

    let candidates = require_unique_tanks(&session.fetch_and_probe()?)?;
    verify_bindings(config, &candidates)?;
    if commands
        .send(Command::RuntimeConnected(generation))
        .is_err()
    {
        return Ok(());
    }
    session.enable_updates()?;

    let mut last_keepalive = Instant::now();
    let connected_at = Instant::now();
    let mut backoff_reset = false;
    while !stop.load(Ordering::Acquire) {
        if !backoff_reset && connected_at.elapsed() >= STABLE_CONNECTION_WINDOW {
            *retry_attempt = 0;
            backoff_reset = true;
        }
        match session.read_packet() {
            Ok(Packet::Values(values)) if !values.is_empty() => {
                if commands
                    .send(Command::RuntimeValues(generation, values))
                    .is_err()
                {
                    return Ok(());
                }
            }
            Ok(Packet::Values(_)) => {}
            Ok(Packet::OutOfService) => return Err(LoxoneError::OutOfService),
            Ok(Packet::KeepAlive) => last_keepalive = Instant::now(),
            Ok(Packet::Text(_) | Packet::Other) => {}
            Err(LoxoneError::Timeout) => {}
            Err(error) => return Err(error),
        }

        if last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
            session.keep_alive()?;
            last_keepalive = Instant::now();
        }
    }
    Ok(())
}

fn retry_delay(attempt: u32) -> Duration {
    let multiplier = 1_u64 << attempt.min(6);
    Duration::from_secs(
        INITIAL_RETRY_DELAY
            .as_secs()
            .saturating_mul(multiplier)
            .min(MAX_RETRY_DELAY.as_secs()),
    )
}

fn wait_for_retry(stop: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    while !stop.load(Ordering::Acquire) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(Duration::from_secs(1)));
    }
    false
}

fn verify_bindings(
    config: &Config,
    candidates: &[crate::probe::TankSensorCandidate],
) -> Result<(), LoxoneError> {
    for candidate in candidates {
        let expected = match candidate.tank {
            crate::probe::TankKind::Fresh => &config.tanks.fresh.state_uuid,
            crate::probe::TankKind::Gray => &config.tanks.gray.state_uuid,
            crate::probe::TankKind::Black => &config.tanks.black.state_uuid,
        };
        if expected != &candidate.state_uuid {
            return Err(LoxoneError::Protocol(format!(
                "{} sensor binding changed; verify the configuration again",
                candidate.tank.sensor_name()
            )));
        }
    }
    Ok(())
}

fn token_needs_refresh(valid_until: u64) -> bool {
    valid_until <= loxone_now().saturating_add(TOKEN_REFRESH_WINDOW)
}

fn loxone_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(LOXONE_EPOCH_UNIX)
}

fn public_error(error: &LoxoneError) -> String {
    match error {
        LoxoneError::Authentication => "Authentication failed".to_owned(),
        LoxoneError::Timeout => "Miniserver did not respond".to_owned(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::TankBinding,
        probe::{TankKind, TankSensorCandidate},
    };

    #[test]
    fn matching_sensor_bindings_are_required() {
        let mut config = Config::default();
        config.tanks.fresh = TankBinding {
            state_uuid: "aaaaaaaa-aaaa-aaaa-aaaaaaaaaaaaaaaa".to_owned(),
            capacity_liters: 0.0,
        };
        let candidate = TankSensorCandidate {
            tank: TankKind::Fresh,
            name: "FW Tank".to_owned(),
            control_uuid: "11111111-1111-1111-1111111111111111".to_owned(),
            state_uuid: config.tanks.fresh.state_uuid.clone(),
            format: "%.1f%%".to_owned(),
        };
        assert!(verify_bindings(&config, std::slice::from_ref(&candidate)).is_ok());

        let mut changed = candidate;
        changed.state_uuid = "bbbbbbbb-bbbb-bbbb-bbbbbbbbbbbbbbbb".to_owned();
        assert!(verify_bindings(&config, &[changed]).is_err());
    }

    #[test]
    fn loxone_epoch_conversion_is_saturating() {
        assert!(loxone_now() > 0);
        assert!(token_needs_refresh(0));
    }

    #[test]
    fn retry_delay_is_exponential_and_bounded() {
        assert_eq!(retry_delay(0), Duration::from_secs(5));
        assert_eq!(retry_delay(1), Duration::from_secs(10));
        assert_eq!(retry_delay(3), Duration::from_secs(40));
        assert_eq!(retry_delay(4), Duration::from_secs(60));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn retries_only_transient_connection_failures() {
        assert!(LoxoneError::OutOfService.is_retryable());
        assert!(LoxoneError::ConnectionClosed.is_retryable());
        assert!(LoxoneError::Timeout.is_retryable());
        assert!(!LoxoneError::Authentication.is_retryable());
        assert!(!LoxoneError::Protocol("invalid response".to_owned()).is_retryable());
    }
}
