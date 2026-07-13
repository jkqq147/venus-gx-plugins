use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::{contract::is_sha256, error::io_error, CoreError, PluginManifest, Runtime};

const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 512;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageExpectation {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

pub(crate) struct PreparedPackage {
    pub manifest: PluginManifest,
    pub sha256: String,
    pub payload: PathBuf,
}

pub(crate) fn prepare_package(
    source: &Path,
    transaction_dir: &Path,
    expectation: &PackageExpectation,
) -> Result<PreparedPackage, CoreError> {
    if !is_sha256(&expectation.sha256) {
        return Err(CoreError::InvalidPackage(
            "expected SHA-256 must contain exactly 64 hexadecimal characters".into(),
        ));
    }
    let expected_sha256 = expectation.sha256.to_ascii_lowercase();
    let copied_package = transaction_dir.join("package.vplugin");
    let actual_sha256 = copy_and_hash(source, &copied_package)?;
    if actual_sha256 != expected_sha256 {
        return Err(CoreError::ChecksumMismatch {
            expected: expected_sha256,
            actual: actual_sha256,
        });
    }

    let payload = transaction_dir.join("payload");
    let manifest = extract_package(&copied_package, &payload)?;
    if manifest.id != expectation.id {
        return Err(CoreError::IdentityMismatch {
            field: "id",
            expected: expectation.id.clone(),
            actual: manifest.id,
        });
    }
    if manifest.version != expectation.version {
        return Err(CoreError::IdentityMismatch {
            field: "version",
            expected: expectation.version.clone(),
            actual: manifest.version,
        });
    }

    Ok(PreparedPackage {
        manifest,
        sha256: expectation.sha256.to_ascii_lowercase(),
        payload,
    })
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<String, CoreError> {
    let mut input = File::open(source).map_err(|error| io_error(source.to_path_buf(), error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| io_error(destination.to_path_buf(), error))?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| io_error(source.to_path_buf(), error))?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > MAX_PACKAGE_BYTES {
            return Err(CoreError::PackageTooLarge {
                limit: MAX_PACKAGE_BYTES,
            });
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| io_error(destination.to_path_buf(), error))?;
    }
    output
        .sync_all()
        .map_err(|error| io_error(destination.to_path_buf(), error))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_package(package: &Path, destination: &Path) -> Result<PluginManifest, CoreError> {
    create_dir_all(destination)?;
    let file = File::open(package).map_err(|error| io_error(package.to_path_buf(), error))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| CoreError::InvalidPackage(error.to_string()))?;
    let mut seen = HashSet::new();
    let mut regular_files = HashSet::new();
    let mut total_size = 0_u64;

    for (index, entry) in entries.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(CoreError::InvalidPackage(format!(
                "archive contains more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let mut entry = entry.map_err(|error| CoreError::InvalidPackage(error.to_string()))?;
        let path = entry
            .path()
            .map_err(|error| CoreError::InvalidPackage(error.to_string()))?
            .into_owned();
        validate_archive_path(&path, entry.header().entry_type().is_dir())?;
        if !seen.insert(path.clone()) {
            return Err(CoreError::InvalidPackage(format!(
                "duplicate archive path: {}",
                path.display()
            )));
        }

        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(CoreError::InvalidPackage(format!(
                "unsupported archive entry type at {}",
                path.display()
            )));
        }
        if path == Path::new("manifest.json") && !entry_type.is_file() {
            return Err(CoreError::InvalidPackage(
                "manifest.json must be a regular file".into(),
            ));
        }

        let target = destination.join(&path);
        if entry_type.is_dir() {
            create_dir_all(&target)?;
            continue;
        }
        total_size = total_size
            .checked_add(entry.header().size().map_err(|error| {
                CoreError::InvalidPackage(format!("{}: {error}", path.display()))
            })?)
            .ok_or_else(|| CoreError::InvalidPackage("archive size overflow".into()))?;
        if total_size > MAX_EXTRACTED_BYTES {
            return Err(CoreError::InvalidPackage(format!(
                "archive expands beyond {MAX_EXTRACTED_BYTES} bytes"
            )));
        }
        if let Some(parent) = target.parent() {
            create_dir_all(parent)?;
        }
        entry
            .unpack(&target)
            .map_err(|error| CoreError::InvalidPackage(format!("{}: {error}", path.display())))?;
        regular_files.insert(path);
    }

    if !regular_files.contains(Path::new("manifest.json")) {
        return Err(CoreError::InvalidPackage(
            "archive does not contain manifest.json".into(),
        ));
    }
    let manifest_path = destination.join("manifest.json");
    let manifest_contents =
        fs::read(&manifest_path).map_err(|error| io_error(manifest_path.clone(), error))?;
    let manifest: PluginManifest = serde_json::from_slice(&manifest_contents)
        .map_err(|error| CoreError::InvalidPackage(format!("manifest.json: {error}")))?;
    manifest
        .validate()
        .map_err(|error| CoreError::InvalidPackage(error.to_string()))?;
    validate_payload(destination, &manifest)?;
    normalize_permissions(destination, &manifest)?;
    Ok(manifest)
}

fn validate_archive_path(path: &Path, is_directory: bool) -> Result<(), CoreError> {
    let text = path
        .to_str()
        .ok_or_else(|| CoreError::InvalidPackage("archive paths must use UTF-8".into()))?;
    if text.is_empty() || text.contains('\\') {
        return Err(CoreError::InvalidPackage(format!(
            "invalid archive path: {}",
            path.display()
        )));
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(CoreError::InvalidPackage(format!(
            "unsafe archive path: {}",
            path.display()
        )));
    }

    let mut components = path.components();
    let first = components.next().and_then(|component| match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    });
    match first {
        Some("manifest.json") if components.next().is_none() => Ok(()),
        Some("bin" | "qml") if is_directory || components.next().is_some() => Ok(()),
        _ => Err(CoreError::InvalidPackage(format!(
            "path is outside the package contract: {}",
            path.display()
        ))),
    }
}

pub(crate) fn validate_payload(root: &Path, manifest: &PluginManifest) -> Result<(), CoreError> {
    require_regular_file(&root.join("manifest.json"))?;
    if let Runtime::NativeService { executable } = &manifest.runtime {
        require_regular_file(&root.join(executable))?;
    }
    for relative_path in [&manifest.ui.settings_page, &manifest.ui.dashboard_component]
        .into_iter()
        .flatten()
    {
        require_regular_file(&root.join(relative_path))?;
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), CoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CoreError::InvalidPackage(format!("required file {}: {error}", path.display()))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::InvalidPackage(format!(
            "required path is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn normalize_permissions(root: &Path, manifest: &PluginManifest) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;

    fn visit(path: &Path) -> Result<(), CoreError> {
        for entry in fs::read_dir(path).map_err(|error| io_error(path.to_path_buf(), error))? {
            let entry = entry.map_err(|error| io_error(path.to_path_buf(), error))?;
            let child = entry.path();
            let metadata =
                fs::symlink_metadata(&child).map_err(|error| io_error(child.clone(), error))?;
            if metadata.is_dir() {
                fs::set_permissions(&child, fs::Permissions::from_mode(0o755))
                    .map_err(|error| io_error(child.clone(), error))?;
                visit(&child)?;
            } else if metadata.is_file() {
                fs::set_permissions(&child, fs::Permissions::from_mode(0o644))
                    .map_err(|error| io_error(child.clone(), error))?;
            } else {
                return Err(CoreError::InvalidPackage(format!(
                    "unsupported extracted path: {}",
                    child.display()
                )));
            }
        }
        Ok(())
    }

    fs::set_permissions(root, fs::Permissions::from_mode(0o755))
        .map_err(|error| io_error(root.to_path_buf(), error))?;
    visit(root)?;
    if let Runtime::NativeService { executable } = &manifest.runtime {
        let path = root.join(executable);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn normalize_permissions(_root: &Path, _manifest: &PluginManifest) -> Result<(), CoreError> {
    Ok(())
}

fn create_dir_all(path: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(path).map_err(|error| io_error(path.to_path_buf(), error))
}
