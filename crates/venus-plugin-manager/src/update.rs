use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use plugin_manager_core::PackageSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    catalog::{cleanup_temporary_downloads, CatalogError, HttpTransport, SystemHttpTransport},
    signing::{CatalogVerifier, SigningError},
};

const MANAGER_ARTIFACT_ID: &str = "plugin-manager";
const RELEASE_SCHEMA_VERSION: u32 = 1;
const MAX_RELEASE_BYTES: u64 = 64 * 1024;
const MAX_BINARY_BYTES: u64 = 8 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerRelease {
    pub schema: u32,
    pub version: String,
    pub binary: PackageSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerUpdateSnapshot {
    pub installed_version: String,
    pub available_version: String,
    pub has_update: bool,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error(transparent)]
    Download(#[from] CatalogError),
    #[error(transparent)]
    Signature(#[from] SigningError),
    #[error("manager release URL must use HTTPS: {0}")]
    InsecureUrl(String),
    #[error("invalid manager release: {0}")]
    InvalidRelease(String),
    #[error("manager update is not available")]
    NotAvailable,
    #[error("manager {available} is not newer than installed version {installed}")]
    NotNewer {
        installed: String,
        available: String,
    },
    #[error("manager binary SHA-256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("downloaded manager reports version {actual}, expected {expected}")]
    VersionMismatch { expected: String, actual: String },
    #[error("manager update command failed: {0}")]
    Command(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub struct ManagerUpdater<T = SystemHttpTransport> {
    release_url: String,
    downloads_dir: PathBuf,
    transport: T,
    verifier: CatalogVerifier,
    release: Option<ManagerRelease>,
}

impl ManagerUpdater<SystemHttpTransport> {
    pub fn new(release_url: impl Into<String>, downloads_dir: impl Into<PathBuf>) -> Self {
        Self::with_transport_and_verifier(
            release_url,
            downloads_dir,
            SystemHttpTransport,
            CatalogVerifier::release().expect("embedded release public key must be valid"),
        )
    }
}

impl<T: HttpTransport> ManagerUpdater<T> {
    pub fn with_transport_and_verifier(
        release_url: impl Into<String>,
        downloads_dir: impl Into<PathBuf>,
        transport: T,
        verifier: CatalogVerifier,
    ) -> Self {
        Self {
            release_url: release_url.into(),
            downloads_dir: downloads_dir.into(),
            transport,
            verifier,
            release: None,
        }
    }

    pub fn initialize(&mut self) -> Result<ManagerUpdateSnapshot, UpdateError> {
        cleanup_temporary_downloads(&self.downloads_dir)?;
        Ok(self.snapshot())
    }

    pub fn refresh(&mut self) -> Result<ManagerUpdateSnapshot, UpdateError> {
        require_https(&self.release_url)?;
        let mut contents = Vec::new();
        self.transport
            .download(&self.release_url, &mut contents, MAX_RELEASE_BYTES)?;
        let release = self.parse_release(&contents)?;
        self.release = Some(release);
        Ok(self.snapshot())
    }

    pub fn snapshot(&self) -> ManagerUpdateSnapshot {
        let installed_version = env!("CARGO_PKG_VERSION").to_owned();
        let available_version = self
            .release
            .as_ref()
            .map(|release| release.version.clone())
            .unwrap_or_default();
        let has_update = self
            .release
            .as_ref()
            .is_some_and(|release| version_is_newer(&release.version, env!("CARGO_PKG_VERSION")));
        ManagerUpdateSnapshot {
            installed_version,
            available_version,
            has_update,
        }
    }

    pub fn apply(&self) -> Result<(), UpdateError> {
        let release = self.release.as_ref().ok_or(UpdateError::NotAvailable)?;
        let installed = env!("CARGO_PKG_VERSION");
        if !version_is_newer(&release.version, installed) {
            return Err(UpdateError::NotNewer {
                installed: installed.into(),
                available: release.version.clone(),
            });
        }

        let candidate = self.download_candidate(release)?;
        let result = verify_candidate_version(&candidate, &release.version).and_then(|_| {
            let status = Command::new(&candidate)
                .args(["apply-manager-update", release.version.as_str()])
                .status()
                .map_err(|error| UpdateError::Command(error.to_string()))?;
            if status.success() {
                Ok(())
            } else {
                Err(UpdateError::Command(format!("exit status {status}")))
            }
        });
        if result.is_err() {
            let _ = fs::remove_file(&candidate);
        }
        result
    }

    fn parse_release(&self, contents: &[u8]) -> Result<ManagerRelease, UpdateError> {
        parse_release(contents, &self.verifier)
    }

    fn download_candidate(&self, release: &ManagerRelease) -> Result<PathBuf, UpdateError> {
        fs::create_dir_all(&self.downloads_dir)
            .map_err(|source| io_error(self.downloads_dir.clone(), source))?;
        let final_path = self
            .downloads_dir
            .join(format!("venus-plugin-manager-{}", release.version));
        let temp_path = self.downloads_dir.join(format!(
            ".manager-update.tmp-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|source| io_error(temp_path.clone(), source))?;
        let mut hasher = Sha256::new();
        let result = (|| {
            {
                let mut destination = HashingWriter {
                    inner: &mut file,
                    hasher: &mut hasher,
                };
                self.transport
                    .download(&release.binary.url, &mut destination, MAX_BINARY_BYTES)?;
            }
            file.sync_all()
                .map_err(|source| io_error(temp_path.clone(), source))?;
            let actual = format!("{:x}", hasher.finalize());
            if actual != release.binary.sha256 {
                return Err(UpdateError::HashMismatch {
                    expected: release.binary.sha256.clone(),
                    actual,
                });
            }
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))
                .map_err(|source| io_error(temp_path.clone(), source))?;
            fs::rename(&temp_path, &final_path)
                .map_err(|source| io_error(final_path.clone(), source))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result.map(|_| final_path)
    }
}

pub fn validate_release(contents: &[u8]) -> Result<ManagerRelease, UpdateError> {
    parse_release(
        contents,
        &CatalogVerifier::release().expect("embedded release public key must be valid"),
    )
}

fn parse_release(
    contents: &[u8],
    verifier: &CatalogVerifier,
) -> Result<ManagerRelease, UpdateError> {
    let release: ManagerRelease = serde_json::from_slice(contents)
        .map_err(|error| UpdateError::InvalidRelease(error.to_string()))?;
    if release.schema != RELEASE_SCHEMA_VERSION {
        return Err(UpdateError::InvalidRelease(format!(
            "unsupported schema version {}",
            release.schema
        )));
    }
    if parse_version(&release.version).is_none() {
        return Err(UpdateError::InvalidRelease(format!(
            "invalid semantic version {}",
            release.version
        )));
    }
    require_https(&release.binary.url)?;
    if release.binary.sha256.len() != 64
        || !release
            .binary
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UpdateError::InvalidRelease(
            "SHA-256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    verifier.verify_artifact(MANAGER_ARTIFACT_ID, &release.version, &release.binary)?;
    Ok(release)
}

fn require_https(url: &str) -> Result<(), UpdateError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(UpdateError::InsecureUrl(url.into()))
    }
}

fn parse_version(version: &str) -> Option<[u64; 3]> {
    let parts = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    parts.try_into().ok()
}

fn version_is_newer(available: &str, installed: &str) -> bool {
    parse_version(available)
        .zip(parse_version(installed))
        .is_some_and(|(available, installed)| available > installed)
}

struct HashingWriter<'a, W> {
    inner: &'a mut W,
    hasher: &'a mut Sha256,
}

impl<W: Write> Write for HashingWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn verify_candidate_version(path: &Path, expected: &str) -> Result<(), UpdateError> {
    let output = Command::new(path)
        .arg("version")
        .output()
        .map_err(|error| UpdateError::Command(error.to_string()))?;
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && actual == expected {
        Ok(())
    } else {
        Err(UpdateError::VersionMismatch {
            expected: expected.into(),
            actual,
        })
    }
}

fn io_error(path: impl Into<PathBuf>, source: io::Error) -> UpdateError {
    UpdateError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, io::Write, sync::Mutex};

    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use tempfile::TempDir;

    use super::*;

    const RELEASE_URL: &str = "https://example.com/manager/release.json";
    const BINARY_URL: &str = "https://example.com/manager.bin";
    const TEST_KEY_ID: &str = "test-key";

    #[derive(Default)]
    struct FakeTransport {
        responses: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl FakeTransport {
        fn insert(&self, url: &str, contents: impl Into<Vec<u8>>) {
            self.responses
                .lock()
                .unwrap()
                .insert(url.into(), contents.into());
        }
    }

    impl HttpTransport for FakeTransport {
        fn download(
            &self,
            url: &str,
            destination: &mut dyn Write,
            limit: u64,
        ) -> Result<u64, CatalogError> {
            let contents = self
                .responses
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| CatalogError::Http {
                    url: url.into(),
                    message: "offline".into(),
                })?;
            if contents.len() as u64 > limit {
                return Err(CatalogError::TooLarge {
                    url: url.into(),
                    limit,
                });
            }
            destination
                .write_all(&contents)
                .map_err(|source| CatalogError::Io {
                    path: PathBuf::from("<test>"),
                    source,
                })?;
            Ok(contents.len() as u64)
        }
    }

    fn verifier() -> CatalogVerifier {
        let public = SigningKey::from_bytes(&[9; 32]).verifying_key();
        CatalogVerifier::from_base64(TEST_KEY_ID, &STANDARD.encode(public.as_bytes())).unwrap()
    }

    fn release(binary: &[u8], version: &str) -> ManagerRelease {
        let sha256 = format!("{:x}", Sha256::digest(binary));
        let key = SigningKey::from_bytes(&[9; 32]);
        let signature = key.sign(&crate::signing::signature_message_parts(
            MANAGER_ARTIFACT_ID,
            version,
            &sha256,
        ));
        ManagerRelease {
            schema: RELEASE_SCHEMA_VERSION,
            version: version.into(),
            binary: PackageSource {
                url: BINARY_URL.into(),
                sha256,
                signature: plugin_manager_core::PackageSignature {
                    key_id: TEST_KEY_ID.into(),
                    ed25519: STANDARD.encode(signature.to_bytes()),
                },
            },
        }
    }

    #[test]
    fn refresh_verifies_release_metadata_in_memory() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let expected = release(b"manager", "9.0.0");
        transport.insert(RELEASE_URL, serde_json::to_vec(&expected).unwrap());
        let mut updater = ManagerUpdater::with_transport_and_verifier(
            RELEASE_URL,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        let snapshot = updater.refresh().unwrap();
        assert_eq!(snapshot.available_version, "9.0.0");
        assert!(snapshot.has_update);
        assert!(!temp.path().join("cache").exists());
    }

    #[test]
    fn changed_release_metadata_keeps_last_verified_value_in_memory() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let mut valid = release(b"manager", "9.0.0");
        transport.insert(RELEASE_URL, serde_json::to_vec(&valid).unwrap());
        let mut updater = ManagerUpdater::with_transport_and_verifier(
            RELEASE_URL,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );
        updater.refresh().unwrap();
        valid.version = "9.0.1".into();
        updater
            .transport
            .insert(RELEASE_URL, serde_json::to_vec(&valid).unwrap());

        assert!(matches!(
            updater.refresh(),
            Err(UpdateError::Signature(SigningError::VerificationFailed(_)))
        ));
        assert_eq!(updater.snapshot().available_version, "9.0.0");
    }

    #[test]
    fn candidate_download_requires_the_signed_sha256() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        transport.insert(BINARY_URL, b"changed".to_vec());
        let updater = ManagerUpdater::with_transport_and_verifier(
            RELEASE_URL,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert!(matches!(
            updater.download_candidate(&release(b"manager", "9.0.0")),
            Err(UpdateError::HashMismatch { .. })
        ));
    }

    #[test]
    fn version_comparison_never_downgrades() {
        assert!(version_is_newer("0.2.0", "0.1.9"));
        assert!(version_is_newer("1.0.0", "0.99.99"));
        assert!(!version_is_newer("0.1.1", "0.1.1"));
        assert!(!version_is_newer("0.1.0", "0.1.1"));
        assert!(!version_is_newer("latest", "0.1.1"));
    }
}
