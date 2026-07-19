use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use plugin_manager_core::{validate_vplugin, Catalog, CatalogEntry, CoreError, PackageExpectation};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::signing::{CatalogVerifier, SigningError};

const MAX_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const DISTRIBUTION_HOST: &str = "venus-gx-plugins.pages.dev";
const DISTRIBUTION_ORIGIN: &str = "https://venus-gx-plugins.pages.dev";
const OPENSSL_PATH: &str = "/usr/bin/openssl";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
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
    #[error("plugin package SHA-256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("invalid plugin package: {0}")]
    Package(#[from] CoreError),
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
        let request_target = distribution_request_target(url)?;
        let mut child = Command::new(OPENSSL_PATH)
            .args([
                "s_client",
                "-quiet",
                "-verify_return_error",
                "-verify_hostname",
                DISTRIBUTION_HOST,
                "-CApath",
                "/etc/ssl/certs",
                "-connect",
                "venus-gx-plugins.pages.dev:443",
                "-servername",
                DISTRIBUTION_HOST,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| http_error(url, format!("could not start OpenSSL: {error}")))?;
        let deadline = ProcessDeadline::new(child.id(), DOWNLOAD_TIMEOUT);
        let request = format!(
            "GET {request_target} HTTP/1.1\r\nHost: {DISTRIBUTION_HOST}\r\nUser-Agent: venus-plugin-manager/{}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
            env!("CARGO_PKG_VERSION")
        );
        let send_result = child
            .stdin
            .take()
            .ok_or_else(|| http_error(url, "OpenSSL stdin was not available"))
            .and_then(|mut stdin| {
                stdin
                    .write_all(request.as_bytes())
                    .map_err(|error| http_error(url, error.to_string()))
            });
        if let Err(error) = send_result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| http_error(url, "OpenSSL stdout was not available"))?;
        let mut reader = BufReader::new(stdout);
        let result = read_distribution_response(&mut reader, destination, url, limit);
        drop(reader);
        if result.is_err() {
            let _ = child.kill();
        }
        let output = child
            .wait_with_output()
            .map_err(|error| http_error(url, format!("could not wait for OpenSSL: {error}")))?;
        let timed_out = deadline.timed_out();
        drop(deadline);
        if timed_out {
            return Err(http_error(url, "HTTPS request timed out after 120 seconds"));
        }
        let copied = result?;
        if output.status.success() {
            Ok(copied)
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(http_error(
                url,
                if message.is_empty() {
                    format!("OpenSSL exited with {}", output.status)
                } else {
                    message
                },
            ))
        }
    }
}

struct ProcessDeadline {
    cancel: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    timed_out: Arc<AtomicBool>,
}

impl ProcessDeadline {
    fn new(pid: u32, duration: Duration) -> Self {
        let (cancel, receiver) = mpsc::channel();
        let timed_out = Arc::new(AtomicBool::new(false));
        let worker_timed_out = Arc::clone(&timed_out);
        let worker = thread::spawn(move || {
            if receiver.recv_timeout(duration) == Err(mpsc::RecvTimeoutError::Timeout) {
                worker_timed_out.store(true, Ordering::Relaxed);
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        });
        Self {
            cancel: Some(cancel),
            worker: Some(worker),
            timed_out,
        }
    }

    fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Relaxed)
    }
}

impl Drop for ProcessDeadline {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn distribution_request_target(url: &str) -> Result<String, CatalogError> {
    let suffix = url
        .strip_prefix(DISTRIBUTION_ORIGIN)
        .ok_or_else(|| http_error(url, "URL is outside the fixed distribution origin"))?;
    if suffix
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        || suffix.contains('#')
    {
        return Err(http_error(url, "invalid distribution URL"));
    }
    if suffix.is_empty() {
        Ok("/".into())
    } else if suffix.starts_with('?') {
        Ok(format!("/{suffix}"))
    } else if suffix.starts_with('/') && !suffix.starts_with("//") {
        Ok(suffix.to_owned())
    } else {
        Err(http_error(
            url,
            "URL is outside the fixed distribution origin",
        ))
    }
}

fn read_distribution_response(
    reader: &mut impl BufRead,
    destination: &mut dyn Write,
    url: &str,
    limit: u64,
) -> Result<u64, CatalogError> {
    let mut used = 0_usize;
    let status = read_http_line(reader, url, &mut used)?;
    let valid_status = status
        .strip_prefix("HTTP/1.1 ")
        .or_else(|| status.strip_prefix("HTTP/1.0 "))
        .is_some_and(|value| value.starts_with("200 ") || value == "200");
    if !valid_status {
        return Err(http_error(url, format!("unexpected HTTP status: {status}")));
    }
    let mut content_length = None;
    loop {
        let line = read_http_line(reader, url, &mut used)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| http_error(url, "invalid HTTP response header"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(http_error(
                url,
                "distribution response must not use Transfer-Encoding",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value
                .trim()
                .parse::<u64>()
                .map_err(|_| http_error(url, "invalid HTTP Content-Length"))?;
            if content_length.is_some_and(|existing| existing != parsed) {
                return Err(http_error(url, "conflicting HTTP Content-Length values"));
            }
            content_length = Some(parsed);
        }
    }
    let length = content_length
        .ok_or_else(|| http_error(url, "distribution response has no Content-Length"))?;
    if length > limit {
        return Err(CatalogError::TooLarge {
            url: url.to_owned(),
            limit,
        });
    }
    let copied = copy_limited(reader.take(length), destination, url, limit)?;
    if copied != length {
        return Err(http_error(url, "HTTP response body ended early"));
    }
    Ok(copied)
}

fn read_http_line(
    reader: &mut impl BufRead,
    url: &str,
    used: &mut usize,
) -> Result<String, CatalogError> {
    let mut line = Vec::new();
    let count = reader
        .read_until(b'\n', &mut line)
        .map_err(|error| http_error(url, error.to_string()))?;
    if count == 0 {
        return Err(http_error(
            url,
            if *used == 0 {
                "HTTPS connection closed before an HTTP response was received"
            } else {
                "HTTP response headers ended early"
            },
        ));
    }
    *used += count;
    if *used > MAX_HTTP_HEADER_BYTES {
        return Err(http_error(url, "HTTP response headers are too large"));
    }
    if !line.ends_with(b"\n") {
        return Err(http_error(url, "HTTP response header line ended early"));
    }
    line.pop();
    if line.ends_with(b"\r") {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| http_error(url, "HTTP response headers are not UTF-8"))
}

fn http_error(url: &str, message: impl Into<String>) -> CatalogError {
    CatalogError::Http {
        url: url.to_owned(),
        message: message.into(),
    }
}

pub struct CatalogClient<T = SystemHttpTransport> {
    catalog_url: String,
    downloads_dir: PathBuf,
    transport: T,
    verifier: CatalogVerifier,
}

impl CatalogClient<SystemHttpTransport> {
    pub fn new(catalog_url: impl Into<String>, downloads_dir: impl Into<PathBuf>) -> Self {
        Self::with_transport(catalog_url, downloads_dir, SystemHttpTransport)
    }
}

impl<T: HttpTransport> CatalogClient<T> {
    pub fn with_transport(
        catalog_url: impl Into<String>,
        downloads_dir: impl Into<PathBuf>,
        transport: T,
    ) -> Self {
        Self::with_transport_and_verifier(
            catalog_url,
            downloads_dir,
            transport,
            CatalogVerifier::release().expect("embedded release public key must be valid"),
        )
    }

    pub fn with_transport_and_verifier(
        catalog_url: impl Into<String>,
        downloads_dir: impl Into<PathBuf>,
        transport: T,
        verifier: CatalogVerifier,
    ) -> Self {
        Self {
            catalog_url: catalog_url.into(),
            downloads_dir: downloads_dir.into(),
            transport,
            verifier,
        }
    }

    pub fn refresh(&self) -> Result<Catalog, CatalogError> {
        require_https(&self.catalog_url)?;
        let mut contents = Vec::new();
        self.transport
            .download(&self.catalog_url, &mut contents, MAX_CATALOG_BYTES)?;
        self.parse_catalog(&contents)
    }

    pub fn download_plugin(
        &self,
        catalog: &Catalog,
        id: &str,
    ) -> Result<(PathBuf, CatalogEntry), CatalogError> {
        cleanup_temporary_downloads(&self.downloads_dir)?;
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
        let mut hasher = Sha256::new();
        let result = (|| {
            {
                let mut destination = HashingWriter {
                    inner: &mut file,
                    hasher: &mut hasher,
                };
                self.transport
                    .download(&entry.package.url, &mut destination, MAX_PACKAGE_BYTES)?;
            }
            file.sync_all()
                .map_err(|source| io_error(temp_path.clone(), source))?;
            let actual = format!("{:x}", hasher.finalize());
            if actual != entry.package.sha256 {
                return Err(CatalogError::HashMismatch {
                    expected: entry.package.sha256.clone(),
                    actual,
                });
            }
            let scratch_root = self.downloads_dir.join(".preflight");
            let validation = validate_vplugin(
                &temp_path,
                &scratch_root,
                &PackageExpectation {
                    id: entry.id.clone(),
                    version: entry.version.clone(),
                    sha256: entry.package.sha256.clone(),
                },
            );
            let _ = fs::remove_dir(&scratch_root);
            validation?;
            fs::rename(&temp_path, &final_path)
                .map_err(|source| io_error(final_path.clone(), source))
        })();
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

pub(crate) fn cleanup_temporary_downloads(directory: &Path) -> Result<(), CatalogError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(directory.to_path_buf(), source)),
    };
    for entry in entries {
        let entry = entry.map_err(|source| io_error(directory.to_path_buf(), source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('.') || !name.contains(".tmp-") {
            continue;
        }
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| io_error(path.clone(), source))?;
        if metadata.file_type().is_file() {
            fs::remove_file(&path).map_err(|source| io_error(path, source))?;
        }
    }
    Ok(())
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
    use flate2::{write::GzEncoder, Compression};
    use plugin_manager_core::{
        CatalogEntry, PackageSource, PluginManifest, PluginSettings, PluginUi, Runtime,
        CATALOG_SCHEMA_VERSION, MANIFEST_SCHEMA_VERSION,
    };
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

    fn package() -> Vec<u8> {
        let manifest = PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            description: "Bluetooth tire pressure monitoring".into(),
            version: "0.1.0".into(),
            runtime: Runtime::NativeService {
                executable: "bin/tpms".into(),
                arguments: Vec::new(),
                companion_executables: Vec::new(),
            },
            settings: PluginSettings {
                enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
            },
            ui: PluginUi::default(),
        };
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents, mode) in [
            (
                "manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
                0o644,
            ),
            ("bin/tpms", b"binary".to_vec(), 0o755),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_slice())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn catalog(package_url: &str, package: &[u8]) -> Catalog {
        let sha256 = format!("{:x}", Sha256::digest(package));
        let key = SigningKey::from_bytes(&[7; 32]);
        let signature = key.sign(&crate::signing::signature_message_parts(
            "tpms", "0.1.0", &sha256,
        ));
        Catalog {
            schema: CATALOG_SCHEMA_VERSION,
            plugins: vec![CatalogEntry {
                id: "tpms".into(),
                name: "TPMS".into(),
                description: "Bluetooth tire pressure monitoring".into(),
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
    fn distribution_transport_accepts_only_the_fixed_origin() {
        assert_eq!(
            distribution_request_target("https://venus-gx-plugins.pages.dev/catalog.json").unwrap(),
            "/catalog.json"
        );
        assert_eq!(
            distribution_request_target("https://venus-gx-plugins.pages.dev?version=1").unwrap(),
            "/?version=1"
        );

        for url in [
            "http://venus-gx-plugins.pages.dev/catalog.json",
            "https://venus-gx-plugins.pages.dev.evil.example/catalog.json",
            "https://example.com/catalog.json",
            "https://venus-gx-plugins.pages.dev//evil.example/catalog.json",
            "https://venus-gx-plugins.pages.dev/catalog.json#fragment",
            "https://venus-gx-plugins.pages.dev/catalog.json\r\nInjected: true",
        ] {
            assert!(distribution_request_target(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn distribution_response_streams_an_exact_content_length() {
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\npackage".as_slice();
        let mut destination = Vec::new();

        let copied = read_distribution_response(
            &mut response,
            &mut destination,
            "https://venus-gx-plugins.pages.dev/package.vplugin",
            16,
        )
        .unwrap();

        assert_eq!(copied, 7);
        assert_eq!(destination, b"package");
    }

    #[test]
    fn distribution_response_rejects_ambiguous_framing() {
        for response in [
            "HTTP/1.1 200 OK\r\n\r\npackage",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\npackage\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nContent-Length: 8\r\n\r\npackage",
        ] {
            let mut reader = response.as_bytes();
            assert!(read_distribution_response(
                &mut reader,
                &mut Vec::new(),
                "https://venus-gx-plugins.pages.dev/package.vplugin",
                16,
            )
            .is_err());
        }
    }

    #[test]
    fn distribution_response_enforces_status_size_and_completeness() {
        let cases = [
            "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\nseventeen-bytes!",
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\nshort",
        ];
        for response in cases {
            let mut reader = response.as_bytes();
            assert!(read_distribution_response(
                &mut reader,
                &mut Vec::new(),
                "https://venus-gx-plugins.pages.dev/package.vplugin",
                16,
            )
            .is_err());
        }
    }

    #[test]
    fn distribution_response_reports_an_early_connection_close() {
        let error = read_distribution_response(
            &mut b"".as_slice(),
            &mut Vec::new(),
            "https://venus-gx-plugins.pages.dev/package.vplugin",
            16,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("HTTPS connection closed before an HTTP response was received"));
    }

    #[test]
    fn refresh_validates_catalog_without_persisting_it() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let expected = catalog("https://example.com/tpms.vplugin", &package());
        transport.insert(catalog_url, serde_json::to_vec(&expected).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert_eq!(client.refresh().unwrap(), expected);
        assert!(!temp.path().join("cache").exists());
    }

    #[test]
    fn invalid_refresh_does_not_create_persistent_state() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let expected = catalog("https://example.com/tpms.vplugin", &package());
        transport.insert(catalog_url, serde_json::to_vec(&expected).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );
        client.refresh().unwrap();
        client.transport.insert(catalog_url, b"not json".to_vec());

        assert!(client.refresh().is_err());
        assert!(!temp.path().join("cache").exists());
    }

    #[test]
    fn rejects_a_catalog_entry_changed_after_signing() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let catalog_url = "https://example.com/plugins.json";
        let mut changed = catalog("https://example.com/tpms.vplugin", &package());
        changed.plugins[0].version = "0.2.0".into();
        transport.insert(catalog_url, serde_json::to_vec(&changed).unwrap());
        let client = CatalogClient::with_transport_and_verifier(
            catalog_url,
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert!(matches!(
            client.refresh(),
            Err(CatalogError::Signature(SigningError::VerificationFailed(_)))
        ));
        assert!(!temp.path().join("cache").exists());
    }

    #[test]
    fn downloads_catalog_package_to_stable_path() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let package_url = "https://example.com/tpms.vplugin";
        let package = package();
        transport.insert(package_url, package.clone());
        let client = CatalogClient::with_transport_and_verifier(
            "https://example.com/plugins.json",
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        let (path, entry) = client
            .download_plugin(&catalog(package_url, &package), "tpms")
            .unwrap();
        assert_eq!(entry.id, "tpms");
        assert_eq!(fs::read(path).unwrap(), package);
    }

    #[test]
    fn package_download_rejects_a_hash_mismatch_before_installation() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let package_url = "https://example.com/tpms.vplugin";
        transport.insert(package_url, b"tampered".to_vec());
        let client = CatalogClient::with_transport_and_verifier(
            "https://example.com/plugins.json",
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert!(matches!(
            client.download_plugin(&catalog(package_url, &package()), "tpms"),
            Err(CatalogError::HashMismatch { .. })
        ));
        assert!(fs::read_dir(temp.path().join("downloads"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn package_download_rejects_invalid_structure_before_stable_publish() {
        let temp = TempDir::new().unwrap();
        let transport = FakeTransport::default();
        let package_url = "https://example.com/tpms.vplugin";
        let invalid = b"not a vplugin".to_vec();
        transport.insert(package_url, invalid.clone());
        let client = CatalogClient::with_transport_and_verifier(
            "https://example.com/plugins.json",
            temp.path().join("downloads"),
            transport,
            verifier(),
        );

        assert!(matches!(
            client.download_plugin(&catalog(package_url, &invalid), "tpms"),
            Err(CatalogError::Package(_))
        ));
        assert!(fs::read_dir(temp.path().join("downloads"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn rejects_insecure_catalog_url() {
        let temp = TempDir::new().unwrap();
        let client = CatalogClient::with_transport_and_verifier(
            "http://example.com/plugins.json",
            temp.path().join("downloads"),
            FakeTransport::default(),
            verifier(),
        );
        assert!(matches!(
            client.refresh(),
            Err(CatalogError::InsecureUrl(_))
        ));
    }

    #[test]
    fn startup_cleanup_removes_only_owned_temporary_downloads() {
        let temp = TempDir::new().unwrap();
        let downloads = temp.path().join("downloads");
        fs::create_dir(&downloads).unwrap();
        fs::write(downloads.join(".manager-update.tmp-1-0"), b"partial").unwrap();
        fs::write(downloads.join(".tpms-0.1.0.tmp-1-1"), b"partial").unwrap();
        fs::write(downloads.join("tpms-0.1.0.vplugin"), b"complete").unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, downloads.join(".linked.tmp-1")).unwrap();

        cleanup_temporary_downloads(&downloads).unwrap();

        assert!(!downloads.join(".manager-update.tmp-1-0").exists());
        assert!(!downloads.join(".tpms-0.1.0.tmp-1-1").exists());
        assert!(downloads.join("tpms-0.1.0.vplugin").exists());
        assert!(downloads.join(".linked.tmp-1").is_symlink());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }
}
