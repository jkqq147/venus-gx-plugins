use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use plugin_manager_core::{CatalogEntry, PackageSignature, PackageSource};
use rand_core::OsRng;
use thiserror::Error;

pub const RELEASE_KEY_ID: &str = "release-2026-01";
pub const RELEASE_PUBLIC_KEY_B64: &str = "uyIIvylKNt2ycAJ7gUOGTD8Bgip+ZdBgJ/9GGEky8yg=";

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid signing key: {0}")]
    InvalidKey(String),
    #[error("catalog entry {id} uses untrusted signing key {key_id}")]
    UntrustedKey { id: String, key_id: String },
    #[error("catalog entry {0} has an invalid signature encoding")]
    InvalidSignatureEncoding(String),
    #[error("catalog entry {0} signature verification failed")]
    VerificationFailed(String),
    #[error("invalid SHA-256: {0}")]
    InvalidSha256(String),
}

#[derive(Clone)]
pub struct CatalogVerifier {
    key_id: String,
    key: VerifyingKey,
}

impl CatalogVerifier {
    pub fn release() -> Result<Self, SigningError> {
        Self::from_base64(RELEASE_KEY_ID, RELEASE_PUBLIC_KEY_B64)
    }

    pub fn from_base64(key_id: &str, public_key: &str) -> Result<Self, SigningError> {
        let bytes = STANDARD
            .decode(public_key)
            .map_err(|error| SigningError::InvalidKey(error.to_string()))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| SigningError::InvalidKey("public key must contain 32 bytes".into()))?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|error| SigningError::InvalidKey(error.to_string()))?;
        Ok(Self {
            key_id: key_id.into(),
            key,
        })
    }

    pub fn verify(&self, entry: &CatalogEntry) -> Result<(), SigningError> {
        self.verify_artifact(&entry.id, &entry.version, &entry.package)
    }

    pub(crate) fn verify_artifact(
        &self,
        id: &str,
        version: &str,
        package: &PackageSource,
    ) -> Result<(), SigningError> {
        if package.signature.key_id != self.key_id {
            return Err(SigningError::UntrustedKey {
                id: id.into(),
                key_id: package.signature.key_id.clone(),
            });
        }
        let bytes = STANDARD
            .decode(&package.signature.ed25519)
            .map_err(|_| SigningError::InvalidSignatureEncoding(id.into()))?;
        let signature = Signature::from_slice(&bytes)
            .map_err(|_| SigningError::InvalidSignatureEncoding(id.into()))?;
        self.key
            .verify_strict(
                &signature_message_parts(id, version, &package.sha256),
                &signature,
            )
            .map_err(|_| SigningError::VerificationFailed(id.into()))
    }
}

pub fn generate_signing_key(path: &Path) -> Result<String, SigningError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let key = SigningKey::generate(&mut OsRng);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    writeln!(file, "{}", encode_hex(&key.to_bytes())).map_err(|source| io_error(path, source))?;
    file.sync_all().map_err(|source| io_error(path, source))?;
    Ok(STANDARD.encode(key.verifying_key().as_bytes()))
}

pub fn sign_catalog_entry(
    private_key: &Path,
    id: &str,
    version: &str,
    sha256: &str,
) -> Result<PackageSignature, SigningError> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SigningError::InvalidSha256(sha256.into()));
    }
    let key = load_signing_key(private_key)?;
    let message = signature_message_parts(id, version, &sha256.to_ascii_lowercase());
    let signature = key.sign(&message);
    Ok(PackageSignature {
        key_id: RELEASE_KEY_ID.into(),
        ed25519: STANDARD.encode(signature.to_bytes()),
    })
}

fn load_signing_key(path: &Path) -> Result<SigningKey, SigningError> {
    let contents = fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    let bytes = decode_hex(contents.trim())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SigningError::InvalidKey("private key must contain 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub(crate) fn signature_message_parts(id: &str, version: &str, sha256: &str) -> Vec<u8> {
    format!("venus-gx-plugins:v1:{id}:{version}:{sha256}").into_bytes()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Result<Vec<u8>, SigningError> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SigningError::InvalidKey(
            "private key must be hexadecimal".into(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("hexadecimal is ASCII");
            u8::from_str_radix(pair, 16)
                .map_err(|error| SigningError::InvalidKey(error.to_string()))
        })
        .collect()
}

fn io_error(path: impl Into<PathBuf>, source: io::Error) -> SigningError {
    SigningError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use plugin_manager_core::{CatalogEntry, PackageSource};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn generated_key_signs_and_verifies_catalog_identity() {
        let temp = TempDir::new().unwrap();
        let key_path = temp.path().join("release.key");
        let public = generate_signing_key(&key_path).unwrap();
        let signature = sign_catalog_entry(&key_path, "tpms", "0.1.0", &"a".repeat(64)).unwrap();
        let entry = CatalogEntry {
            id: "tpms".into(),
            name: "TPMS".into(),
            description: "Bluetooth tire pressure monitoring".into(),
            version: "0.1.0".into(),
            package: PackageSource {
                url: "https://example.com/tpms.vplugin".into(),
                sha256: "a".repeat(64),
                signature,
            },
        };
        let verifier = CatalogVerifier::from_base64(RELEASE_KEY_ID, &public).unwrap();
        verifier.verify(&entry).unwrap();

        let mut changed = entry;
        changed.version = "0.2.0".into();
        assert!(matches!(
            verifier.verify(&changed),
            Err(SigningError::VerificationFailed(_))
        ));
    }
}
