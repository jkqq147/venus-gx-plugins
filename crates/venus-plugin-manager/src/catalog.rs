use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use plugin_manager_core::{Catalog, CatalogEntry};
use thiserror::Error;

use crate::signing::{CatalogVerifier, SigningError};

const MAX_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("catalog URL must use HTTPS: {0}")]
    InsecureUrl(String),
    #[error("HTTP request failed for {url}: {message}")]
    Http { url: String, message: String },
    #[error("download from {url} exceeds the {limit} byte size limit")]
    TooLarge { url: String, limit: u64 },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid catalog: {0}")]
    InvalidCatalog(String),
    #[error("catalog does not contain plugin {0}")]
    MissingPlugin(String),
    #[error("catalog signature is invalid: {0}")]
    Signature(#[from] SigningError),
}

pub trait HttpTransport: Send + Sync {
    fn download(
        &self,
        url: &str,
        destination: &mut dyn Write,
        limit: u64,
    ) -> Result<u64, CatalogError>;
}

#[derive(Debug, Clone, Default)]
pub struct SystemHttpTransport;

impl HttpTransport for SystemHttpTransport {
    fn download(
        &self,
        url: &str,
        destination: &mut dyn Write,
        limit: u64,
    ) -> Result<u64, CatalogError> {
        require_https(url)?;
        let mut child = Command::new("curl")
            .args([
                "--ipv4",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--retry",
                "3",
                "--retry-delay",
                "0",
                "--retry-max-time",
                "45",
                "--retry-connrefused",
                "--connect-timeout",
                "8",
                "--max-time",
                "120",
            ])
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| CatalogError::Http {
                url: url.to_owned(),
                message: format!("could not start curl: {error}"),
            })?;
        let stdout = child.stdout.take().ok_or_else(|| CatalogError::Http {
            url: url.to_owned(),
            message: "curl stdout was not available".into(),
        })?;
        let copied = match copy_limited(stdout, destination, url, limit) {
            Ok(copied) => copied,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let output = child
            .wait_with_output()
            .map_err(|error| CatalogError::Http {
                url: url.to_owned(),
                message: format!("could not wait for curl: {error}"),
            })?;
        if output.status.success() {
            Ok(copied)
        } else {
            Err(CatalogError::Http {
                url: url.to_owned(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

pub struct CatalogClient<T = SystemHttpTransport> {
    catalog_url: String,
    cache_path: PathBuf,
    downloads_dir: PathBuf,
    transport: T,
    verifier: CatalogVerifier,
}

impl CatalogClient<SystemHttpTransport> {
    pub fn new(
        catalog_url: impl Into<String>,
        cache_path: impl Into<PathBuf>,
        downloads_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::with_transport(catalog_url, cache_path, downloads_dir, SystemHttpTransport)
    }
}

impl<T: HttpTransport> CatalogClient<T> {
    pub fn with_transport(
        catalog_url: impl Into<String>,
        cache_path: impl Into<PathBuf>,
        downloads_dir: impl Into<PathBuf>,
        transport: T,
    ) -> Self {
        Self::with_transport_and_verifier(
            catalog_url,
            cache_path,
            downloads_dir,
            transport,
            CatalogVerifier::release().expect("embedded release public key must be valid"),
        )
    }

    pub fn with_transport_and_verifier(
        catalog_url: impl Into<String>,
        cache_path: impl Into<PathBuf>,
        downloads_dir: impl Into<PathBuf>,
        transport: T,
        verifier: CatalogVerifier,
    ) -> Self {
        Self {
            catalog_url: catalog_url.into(),
            cache_path: cache_path.into(),
            downloads_dir: downloads_dir.into(),
            transport,
            verifier,
        }
    }

    pub fn load_cached(&self) -> Result<Option<Catalog>, CatalogError> {
        if !self.cache_path.exists() {
            return Ok(None);
        }
        let metadata = fs::metadata(&self.cache_path)
            .map_err(|source| io_error(self.cache_path.clone(), source))?;
        if metadata.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge {
                url: self.cache_path.display().to_string(),
                limit: MAX_CATALOG_BYTES,
            });
        }
        let contents = fs::read(&self.cache_path)
            .map_err(|source| io_error(self.cache_path.clone(), source))?;
        self.parse_catalog(&contents).map(Some)
    }

    pub fn refresh(&self) -> Result<Catalog, CatalogError> {
        require_https(&self.catalog_url)?;
        let mut contents = Vec::new();
        self.transport
            .download(&self.catalog_url, &mut contents, MAX_CATALOG_BYTES)?;
        let catalog = self.parse_catalog(&contents)?;
        write_atomic(&self.cache_path, &contents)?;
        Ok(catalog)
    }

    pub fn download_plugin(
        &self,
        catalog: &Catalog,
        id: &str,
    ) -> Result<(PathBuf, CatalogEntry), CatalogError> {
        let entry = catalog
            .plugins
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| CatalogError::MissingPlugin(id.to_owned()))?;
        self.verifier.verify(&entry)?;
        require_https(&entry.package.url)?;
        fs::create_dir_all(&self.downloads_dir)
            .map_err(|source| io_error(self.downloads_dir.clone(), source))?;
        let final_path = self
            .downloads_dir
            .join(format!("{}-{}.vplugin", entry.id, entry.version));
        let temp_path = self.downloads_dir.join(format!(
            ".{}-{}.tmp-{}",
            entry.id,
            entry.version,
            next_suffix()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|source| io_error(temp_path.clone(), source))?;
        let result = self
            .transport
            .download(&entry.package.url, &mut file, MAX_PACKAGE_BYTES)
            .and_then(|_| {
                file.sync_all()
                    .map_err(|source| io_error(temp_path.clone(), source))
            })
            .and_then(|_| {
                fs::rename(&temp_path, &final_path)
                    .map_err(|source| io_error(final_path.clone(), source))
            });
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result?;
        Ok((final_path, entry))
    }

    fn parse_catalog(&self, contents: &[u8]) -> Result<Catalog, CatalogError> {
        let catalog: Catalog = serde_json::from_slice(contents)
            .map_err(|error| CatalogError::InvalidCatalog(error.to_string()))?;
        catalog
            .validate()
            .map_err(|error| CatalogError::InvalidCatalog(error.to_string()))?;
        for entry in &catalog.plugins {
            self.verifier.verify(entry)?;
        }
        Ok(catalog)
    }
}

fn require_https(url: &str) -> Result<(), CatalogError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(CatalogError::InsecureUrl(url.to_owned()))
    }
}

fn copy_limited(
    mut source: impl Read,
    destination: &mut dyn Write,
    url: &str,
    limit: u64,
) -> Result<u64, CatalogError> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|error| CatalogError::Http {
                url: url.to_owned(),
                message: error.to_string(),
            })?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > limit {
            return Err(CatalogError::TooLarge {
                url: url.to_owned(),
                limit,
            });
        }
        destination
            .write_all(&buffer[..count])
            .map_err(|source| CatalogError::Io {
                path: PathBuf::from("<download destination>"),
                source,
            })?;
    }
    Ok(copied)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), CatalogError> {
    let parent = path.parent().ok_or_else(|| {
        CatalogError::InvalidCatalog("catalog cache path has no parent directory".into())
    })?;
    fs::create_dir_all(parent).map_err(|source| io_error(parent.to_path_buf(), source))?;
    let temp_path = parent.join(format!(".catalog.tmp-{}", next_suffix()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(|source| io_error(temp_path.clone(), source))?;
    let result = file
        .write_all(contents)
        .map_err(|source| io_error(temp_path.clone(), source))
        .and_then(|_| {
            file.sync_all()
                .map_err(|source| io_error(temp_path.clone(), source))
        })
        .and_then(|_| {
            fs::rename(&temp_path, path).map_err(|source| io_error(path.to_path_buf(), source))
        });
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn next_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn io_error(path: PathBuf, source: io::Error) -> CatalogError {
    CatalogError::Io { path, source }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use plugin_manager_core::{CatalogEntry, PackageSource, SCHEMA_VERSION};
    use tempfile::TempDir;

    use super::*;

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
                .insert(url.to_owned(), contents.into());
        }
    }

    impl HttpTransport for FakeTransport {
        fn download(
            &self,
            url: &str,
            destination: &mut dyn Write,
            limit: u64,
        ) -> Result<u64, CatalogError> {
            require_https(url)?;
            let contents = self
                .responses
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| CatalogError::Http {
                    url: url.to_owned(),
                    message: "offline".into(),
                })?;
            copy_limited(contents.as_slice(), destination, url, limit)
        }
    }

    fn catalog(package_url: &str) -> Catalog {
        let sha256 = "0".repeat(64);
        let key = SigningKey::from_bytes(&[7; 32]);
        let signature = key.sign(&crate::signing::signature_message_parts(
            "tpms", "0.1.0", &sha256,
        ));
        Catalog {
            schema: SCHEMA_VERSION,
            plugins: vec![CatalogEntry {
                id: "tpms".into(),
                name: "TPMS".into(),
                version: "0.1.0".into(),
                package: PackageSource {
                    url: package_url.into(),
                    sha256,
                    signature: plugin_manager_core::PackageSignature {
                        key_id: TEST_KEY_ID.into(),
                        ed25519: STANDARD.encode(signature.to_bytes()),
                    },
                },
            }],
        }
    }

    fn verifier() -> CatalogVerifier {
        let public = SigningKey::from_bytes(&[7; 32]).verifying_key();
        CatalogVerifier::from_base64(TEST_KEY_ID, &STANDARD.encode(public.as_bytes())).unwrap()
    }

    #[test]
    fn refresh_validates_and_atomically_caches_catalog() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let expected = catalog("https://example.com/tpms.vplugin");
        transport.insert(catalog_url, serde_json::to_vec(&expected).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("cache/catalog.json"),
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert_eq!(client.refresh().unwrap(), expected);
        assert_eq!(client.load_cached().unwrap(), Some(expected));
    }

    #[test]
    fn invalid_refresh_preserves_last_valid_cache() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let expected = catalog("https://example.com/tpms.vplugin");
        transport.insert(catalog_url, serde_json::to_vec(&expected).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("cache/catalog.json"),
            temp.path().join("downloads"),
            transport,
            verifier(),
        );
        client.refresh().unwrap();
        client.transport.insert(catalog_url, b"not json".to_vec());

        assert!(client.refresh().is_err());
        assert_eq!(client.load_cached().unwrap(), Some(expected));
    }

    #[test]
    fn rejects_a_catalog_entry_changed_after_signing() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let mut changed = catalog("https://example.com/tpms.vplugin");
        changed.plugins[0].version = "0.2.0".into();
        transport.insert(catalog_url, serde_json::to_vec(&changed).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("cache/catalog.json"),
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert!(matches!(
            client.refresh(),
            Err(CatalogError::Signature(SigningError::VerificationFailed(_)))
        ));
        assert!(!temp.path().join("cache/catalog.json").exists());
    }

    #[test]
    fn downloads_catalog_package_to_stable_path() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let package_url = "https://example.com/tpms.vplugin";
        transport.insert(package_url, b"package".to_vec());
        let client = CatalogClient::with_transport_and_verifier(
            "https://example.com/plugins.json",
            temp.path().join("cache/catalog.json"),
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        let (path, entry) = client
            .download_plugin(&catalog(package_url), "tpms")
            .unwrap();
        assert_eq!(entry.id, "tpms");
        assert_eq!(fs::read(path).unwrap(), b"package");
    }

    #[test]
    fn rejects_insecure_catalog_url() {
        let temp = TempDir::new().unwrap();
        let client = CatalogClient::with_transport_and_verifier(
            "http://example.com/plugins.json",
            temp.path().join("cache/catalog.json"),
            temp.path().join("downloads"),
            FakeTransport::default(),
            verifier(),
        );
        assert!(matches!(
            client.refresh(),
            Err(CatalogError::InsecureUrl(_))
        ));
    }
}
