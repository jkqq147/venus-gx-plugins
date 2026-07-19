use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rand_core::{OsRng, RngCore};
use serde::Serialize;
use thiserror::Error;
use toml::Value;
use zeroize::Zeroize;

pub const MAX_SERVICES: usize = 8;
pub const DEFAULT_SERVER_PORT: u16 = 2333;
const DEVICE_MARKER: &str = "# venus-gx-device = ";
const TOKEN_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq)]
pub struct ManagedConfig {
    pub server_host: String,
    pub server_port: u16,
    pub device_name: String,
    pub token: String,
    pub services: Vec<ServiceConfig>,
}

impl std::fmt::Debug for ManagedConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedConfig")
            .field("server_host", &self.server_host)
            .field("server_port", &self.server_port)
            .field("device_name", &self.device_name)
            .field("token", &"[redacted]")
            .field("services", &self.services)
            .finish()
    }
}

impl Drop for ManagedConfig {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    pub slug: String,
    pub local_host: String,
    pub local_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    Missing,
    Managed,
    Advanced,
    Invalid,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub mode: LoadMode,
    pub draft: Option<ManagedConfig>,
    pub detail: &'static str,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("server address is required")]
    MissingServer,
    #[error("invalid server address")]
    InvalidServer,
    #[error("device name is required")]
    MissingDeviceName,
    #[error("invalid device name")]
    InvalidDeviceName,
    #[error("token is required")]
    MissingToken,
    #[error("invalid token")]
    InvalidToken,
    #[error("too many services")]
    TooManyServices,
    #[error("at least one service is required")]
    MissingService,
    #[error("invalid service")]
    InvalidService,
    #[error("duplicate service name")]
    DuplicateService,
}

#[derive(Debug)]
enum ParseFailure {
    Advanced(&'static str),
    Invalid(&'static str),
}

impl Default for ManagedConfig {
    fn default() -> Self {
        Self {
            server_host: String::new(),
            server_port: DEFAULT_SERVER_PORT,
            device_name: String::new(),
            token: generate_token(),
            services: Vec::new(),
        }
    }
}

impl ManagedConfig {
    pub fn load(path: &Path) -> io::Result<LoadedConfig> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadedConfig {
                    mode: LoadMode::Missing,
                    draft: Some(Self::default()),
                    detail: "not-configured",
                });
            }
            Err(error) => return Err(error),
        };

        Ok(match parse(&contents) {
            Ok(config) => LoadedConfig {
                mode: LoadMode::Managed,
                draft: Some(config),
                detail: "ready",
            },
            Err(ParseFailure::Advanced(detail)) => LoadedConfig {
                mode: LoadMode::Advanced,
                draft: None,
                detail,
            },
            Err(ParseFailure::Invalid(detail)) => LoadedConfig {
                mode: LoadMode::Invalid,
                draft: None,
                detail,
            },
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server_host.is_empty() {
            return Err(ConfigError::MissingServer);
        }
        if !valid_host(&self.server_host) || self.server_port == 0 {
            return Err(ConfigError::InvalidServer);
        }
        if self.device_name.is_empty() {
            return Err(ConfigError::MissingDeviceName);
        }
        if !valid_identifier(&self.device_name, 24) {
            return Err(ConfigError::InvalidDeviceName);
        }
        if self.token.is_empty() {
            return Err(ConfigError::MissingToken);
        }
        if self.token.len() > 256
            || self
                .token
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(ConfigError::InvalidToken);
        }
        if self.services.len() > MAX_SERVICES {
            return Err(ConfigError::TooManyServices);
        }
        if self.services.is_empty() {
            return Err(ConfigError::MissingService);
        }

        let mut names = BTreeSet::new();
        for service in &self.services {
            if !valid_identifier(&service.slug, 32)
                || !valid_host(&service.local_host)
                || service.local_port == 0
            {
                return Err(ConfigError::InvalidService);
            }
            if !names.insert(service.generated_name(&self.device_name)) {
                return Err(ConfigError::DuplicateService);
            }
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.server_host = normalize_host(&self.server_host);
        self.device_name = normalize_identifier(&self.device_name);
        self.token = self.token.trim().to_owned();
        for service in &mut self.services {
            service.slug = normalize_identifier(&service.slug);
            service.local_host = normalize_host(&service.local_host);
        }
    }

    pub fn save_if_changed(&self, path: &Path) -> io::Result<bool> {
        self.validate().map_err(invalid_data)?;
        let encoded = self.encode().map_err(invalid_data)?;
        if fs::read(path).is_ok_and(|current| current == encoded.as_bytes()) {
            ensure_private_file(path)?;
            return Ok(false);
        }

        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing config parent"))?;
        ensure_private_directory(parent)?;
        if path.exists() {
            ensure_private_file(path)?;
        }

        let temporary = temporary_path(path);
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(encoded.as_bytes())?;
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

    pub fn encode(&self) -> Result<String, toml::ser::Error> {
        let services = self
            .services
            .iter()
            .map(|service| {
                (
                    service.generated_name(&self.device_name),
                    OutputService {
                        local_addr: format_host_port(&service.local_host, service.local_port),
                    },
                )
            })
            .collect();
        let output = OutputRoot {
            client: OutputClient {
                remote_addr: format_host_port(&self.server_host, self.server_port),
                default_token: self.token.clone(),
                services,
            },
        };
        let body = toml::to_string_pretty(&output)?;
        Ok(format!(
            "# Managed by Venus GX Plugins.\n{DEVICE_MARKER}\"{}\"\n\n{body}",
            self.device_name
        ))
    }
}

impl ServiceConfig {
    pub fn generated_name(&self, device_name: &str) -> String {
        format!("{}_{}", self.slug, device_name)
    }

    pub fn summary(&self) -> String {
        format_host_port(&self.local_host, self.local_port)
    }
}

#[derive(Serialize)]
struct OutputRoot {
    client: OutputClient,
}

#[derive(Serialize)]
struct OutputClient {
    remote_addr: String,
    default_token: String,
    services: BTreeMap<String, OutputService>,
}

#[derive(Serialize)]
struct OutputService {
    local_addr: String,
}

fn parse(contents: &str) -> Result<ManagedConfig, ParseFailure> {
    let root: Value = contents
        .parse()
        .map_err(|_| ParseFailure::Invalid("invalid-toml"))?;
    let root = root
        .as_table()
        .ok_or(ParseFailure::Invalid("invalid-root"))?;
    if root.keys().any(|key| key != "client") {
        return Err(ParseFailure::Advanced("unsupported-options"));
    }
    let client = root
        .get("client")
        .and_then(Value::as_table)
        .ok_or(ParseFailure::Invalid("missing-client"))?;
    if client
        .keys()
        .any(|key| !matches!(key.as_str(), "remote_addr" | "default_token" | "services"))
    {
        return Err(ParseFailure::Advanced("unsupported-options"));
    }

    let (server_host, server_port) = client
        .get("remote_addr")
        .and_then(Value::as_str)
        .and_then(parse_host_port)
        .ok_or(ParseFailure::Invalid("invalid-server"))?;
    let default_token = client.get("default_token").and_then(Value::as_str);
    let services = client
        .get("services")
        .and_then(Value::as_table)
        .ok_or(ParseFailure::Invalid("missing-services"))?;
    if services.len() > MAX_SERVICES {
        return Err(ParseFailure::Advanced("too-many-services"));
    }

    let marker = parse_device_marker(contents);
    let inferred = infer_device_name(services.keys().map(String::as_str));
    let device_name = marker
        .or(inferred)
        .ok_or(ParseFailure::Advanced("service-names"))?;
    if !valid_identifier(&device_name, 24) {
        return Err(ParseFailure::Advanced("service-names"));
    }

    let suffix = format!("_{device_name}");
    if services.is_empty() {
        return Err(ParseFailure::Invalid("missing-services"));
    }
    let mut imported = Vec::with_capacity(services.len());
    let mut resolved_token: Option<String> = None;
    for (name, value) in services {
        let table = value
            .as_table()
            .ok_or(ParseFailure::Invalid("invalid-service"))?;
        if table
            .keys()
            .any(|key| !matches!(key.as_str(), "local_addr" | "token"))
        {
            return Err(ParseFailure::Advanced("unsupported-options"));
        }
        let slug = name
            .strip_suffix(&suffix)
            .filter(|value| valid_identifier(value, 32))
            .ok_or(ParseFailure::Advanced("service-names"))?;
        let (local_host, local_port) = table
            .get("local_addr")
            .and_then(Value::as_str)
            .and_then(parse_host_port)
            .ok_or(ParseFailure::Invalid("invalid-service"))?;
        let token = table
            .get("token")
            .and_then(Value::as_str)
            .or(default_token)
            .ok_or(ParseFailure::Invalid("missing-token"))?;
        if resolved_token
            .as_ref()
            .is_some_and(|current| current != token)
        {
            return Err(ParseFailure::Advanced("mixed-tokens"));
        }
        resolved_token = Some(token.to_owned());
        imported.push(ServiceConfig {
            slug: slug.to_owned(),
            local_host,
            local_port,
        });
    }

    let token = resolved_token
        .or_else(|| default_token.map(str::to_owned))
        .ok_or(ParseFailure::Invalid("missing-token"))?;
    let mut config = ManagedConfig {
        server_host,
        server_port,
        device_name,
        token,
        services: imported,
    };
    config.normalize();
    config
        .validate()
        .map_err(|_| ParseFailure::Invalid("invalid-config"))?;
    Ok(config)
}

fn parse_device_marker(contents: &str) -> Option<String> {
    contents.lines().take(8).find_map(|line| {
        let value = line.strip_prefix(DEVICE_MARKER)?.trim();
        value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned)
    })
}

fn infer_device_name<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut suffix = None;
    for name in names {
        let candidate = name.rsplit_once('_')?.1;
        if suffix.is_some_and(|current| current != candidate) {
            return None;
        }
        suffix = Some(candidate);
    }
    suffix.map(str::to_owned)
}

fn parse_host_port(value: &str) -> Option<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        let port = port.parse().ok().filter(|value| *value != 0)?;
        return valid_host(host).then(|| (host.to_owned(), port));
    }
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse().ok().filter(|value| *value != 0)?;
    valid_host(host).then(|| (normalize_host(host), port))
}

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn normalize_host(value: &str) -> String {
    value.trim().trim_matches(['[', ']']).to_ascii_lowercase()
}

fn normalize_identifier(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub fn generate_token() -> String {
    let mut random = [0_u8; 8];
    OsRng.fill_bytes(&mut random);
    let mut token = String::with_capacity(9);
    for (index, byte) in random.into_iter().enumerate() {
        if index == 4 {
            token.push('-');
        }
        token.push(TOKEN_ALPHABET[usize::from(byte & 0x1f)] as char);
    }
    token
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
            "config path is not a regular file",
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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

    const LEGACY: &str = r#"[client]
remote_addr = "106.55.191.108:2333"

[client.services.loxone_sn1350]
token = "7K4M-2D9Q"
local_addr = "192.168.50.2:80"

[client.services.hikvision_sn1350]
token = "7K4M-2D9Q"
local_addr = "192.168.50.7:8000"
"#;

    #[test]
    fn imports_legacy_per_service_tokens_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.toml");
        fs::write(&path, LEGACY).unwrap();
        let before = fs::read(&path).unwrap();

        let loaded = ManagedConfig::load(&path).unwrap();

        assert_eq!(loaded.mode, LoadMode::Managed);
        let config = loaded.draft.unwrap();
        assert_eq!(config.server_host, "106.55.191.108");
        assert_eq!(config.server_port, 2333);
        assert_eq!(config.device_name, "sn1350");
        assert_eq!(config.token, "7K4M-2D9Q");
        assert_eq!(config.services.len(), 2);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn managed_save_is_atomic_private_and_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config/client.toml");
        let config = ManagedConfig {
            server_host: "106.55.191.108".into(),
            server_port: 2333,
            device_name: "sn1350".into(),
            token: "7K4M-2D9Q".into(),
            services: vec![ServiceConfig {
                slug: "loxone".into(),
                local_host: "192.168.50.2".into(),
                local_port: 80,
            }],
        };

        assert!(config.save_if_changed(&path).unwrap());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!config.save_if_changed(&path).unwrap());
        assert_eq!(ManagedConfig::load(&path).unwrap().draft.unwrap(), config);
        assert!(fs::read_to_string(path)
            .unwrap()
            .contains("default_token = \"7K4M-2D9Q\""));
    }

    #[test]
    fn advanced_options_are_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.toml");
        fs::write(
            &path,
            LEGACY.replace(
                "remote_addr = \"106.55.191.108:2333\"",
                "remote_addr = \"106.55.191.108:2333\"\nretry_interval = 3",
            ),
        )
        .unwrap();

        let loaded = ManagedConfig::load(&path).unwrap();
        assert_eq!(loaded.mode, LoadMode::Advanced);
        assert_eq!(loaded.detail, "unsupported-options");
        assert!(loaded.draft.is_none());
    }

    #[test]
    fn mixed_tokens_are_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.toml");
        fs::write(&path, LEGACY.replacen("7K4M-2D9Q", "OTHER", 1)).unwrap();

        let loaded = ManagedConfig::load(&path).unwrap();
        assert_eq!(loaded.mode, LoadMode::Advanced);
        assert_eq!(loaded.detail, "mixed-tokens");
    }

    #[test]
    fn generated_token_has_the_requested_short_format() {
        let token = generate_token();
        assert_eq!(token.len(), 9);
        assert_eq!(token.as_bytes()[4], b'-');
        assert!(token
            .bytes()
            .filter(|byte| *byte != b'-')
            .all(|byte| TOKEN_ALPHABET.contains(&byte)));
    }

    #[test]
    fn marker_preserves_device_names_with_underscores() {
        let mut config = ManagedConfig {
            server_host: "tunnel.example.com".into(),
            server_port: 2333,
            device_name: "boat_one".into(),
            token: "7K4M-2D9Q".into(),
            services: vec![ServiceConfig {
                slug: "homeassistant".into(),
                local_host: "192.168.50.100".into(),
                local_port: 8123,
            }],
        };
        config.normalize();
        let encoded = config.encode().unwrap();
        let parsed = parse(&encoded).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn imported_tokens_remain_case_sensitive() {
        let parsed = parse(&LEGACY.replace("7K4M-2D9Q", "Case-Sensitive")).unwrap();
        assert_eq!(parsed.token, "Case-Sensitive");
    }
}
