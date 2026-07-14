use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const CONFIG_SCHEMA: u32 = 1;
pub const CREDENTIALS_SCHEMA: u32 = 1;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub miniserver: MiniserverConfig,
    pub tanks: TankBindings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MiniserverConfig {
    pub host: String,
    pub username: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TankBindings {
    pub fresh: TankBinding,
    pub gray: TankBinding,
    pub black: TankBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TankBinding {
    pub state_uuid: String,
    pub capacity_liters: f64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub schema: u32,
    pub client_uuid: String,
    pub token: String,
    pub valid_until: u64,
    pub token_rights: u64,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("schema", &self.schema)
            .field("client_uuid", &self.client_uuid)
            .field("token", &"[redacted]")
            .field("valid_until", &self.valid_until)
            .field("token_rights", &self.token_rights)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            miniserver: MiniserverConfig::default(),
            tanks: TankBindings::default(),
        }
    }
}

impl Default for TankBinding {
    fn default() -> Self {
        Self {
            state_uuid: String::new(),
            capacity_liters: 0.0,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Self> {
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let config: Self = serde_json::from_slice(&contents).map_err(invalid_data)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save_if_changed(&self, path: &Path) -> io::Result<bool> {
        self.validate()?;
        save_private_json_if_changed(self, path)
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema != CONFIG_SCHEMA {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported config schema {}", self.schema),
            ));
        }
        if (!self.miniserver.host.is_empty()
            && (self.miniserver.host.len() > 253
                || !self.miniserver.host.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                })))
            || self.miniserver.username.len() > 64
            || self
                .miniserver
                .username
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Loxone Miniserver configuration",
            ));
        }
        for binding in [&self.tanks.fresh, &self.tanks.gray, &self.tanks.black] {
            if !binding.capacity_liters.is_finite()
                || !(0.0..=100_000.0).contains(&binding.capacity_liters)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tank capacity must be between 0 and 100000 litres",
                ));
            }
            if binding.state_uuid.len() > 80
                || !binding
                    .state_uuid
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid Loxone state UUID",
                ));
            }
        }
        Ok(())
    }
}

impl Credentials {
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let credentials: Self = serde_json::from_slice(&contents).map_err(invalid_data)?;
        credentials.validate()?;
        Ok(Some(credentials))
    }

    pub fn save_if_changed(&self, path: &Path) -> io::Result<bool> {
        self.validate()?;
        save_private_json_if_changed(self, path)
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema != CREDENTIALS_SCHEMA {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported credentials schema {}", self.schema),
            ));
        }
        if !valid_loxone_uuid(&self.client_uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Loxone client UUID",
            ));
        }
        if self.token.is_empty() || self.token.len() > 8192 || self.token.contains(['\r', '\n']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Loxone token",
            ));
        }
        Ok(())
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

fn save_private_json_if_changed<T: Serialize>(value: &T, path: &Path) -> io::Result<bool> {
    let mut encoded = serde_json::to_vec_pretty(value).map_err(invalid_data)?;
    encoded.push(b'\n');
    if fs::read(path).is_ok_and(|current| current == encoded) {
        ensure_private_file(path)?;
        return Ok(false);
    }

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing config parent"))?;
    ensure_private_directory(parent)?;
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        Ok(true)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "config root is not a directory",
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credentials path is not a regular file",
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn valid_loxone_uuid(value: &str) -> bool {
    let lengths = [8, 4, 4, 16];
    let mut parts = value.split('-');
    lengths.into_iter().all(|length| {
        parts.next().is_some_and(|part| {
            part.len() == length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && parts.next().is_none()
}

fn temporary_path(path: &Path) -> PathBuf {
    let suffix = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("tmp-{}-{suffix}", std::process::id()))
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_config_is_not_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let config = Config::default();

        assert!(config.save_if_changed(&path).unwrap());
        let first = fs::read(&path).unwrap();
        assert!(!config.save_if_changed(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), first);
    }

    #[test]
    fn live_values_cannot_enter_persistent_config() {
        let encoded = serde_json::to_string(&Config::default()).unwrap();
        for forbidden in ["level", "remaining", "last_seen", "connected", "value"] {
            assert!(!encoded.contains(forbidden));
        }
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("token"));
    }

    #[test]
    fn credentials_are_separate_and_not_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.json");
        let credentials = Credentials {
            schema: CREDENTIALS_SCHEMA,
            client_uuid: "098802e1-02b4-603c-ffffeee000d80cfd".to_owned(),
            token: "secret-token".to_owned(),
            valid_until: 123,
            token_rights: 4,
        };

        assert!(credentials.save_if_changed(&path).unwrap());
        assert!(!credentials.save_if_changed(&path).unwrap());
        assert_eq!(Credentials::load(&path).unwrap(), Some(credentials));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn invalid_capacity_is_rejected() {
        let mut config = Config::default();
        config.tanks.fresh.capacity_liters = f64::NAN;
        let directory = tempfile::tempdir().unwrap();
        let error = config
            .save_if_changed(&directory.path().join("settings.json"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
