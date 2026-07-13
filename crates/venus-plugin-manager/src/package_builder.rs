use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use flate2::{Compression, GzBuilder};
use plugin_manager_core::{PluginManifest, Runtime};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltPackage {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("package source contains an unsupported path: {0}")]
    UnsupportedPath(PathBuf),
    #[error("package source path is not a regular file or directory: {0}")]
    UnsupportedType(PathBuf),
    #[error("required package file is missing: {0}")]
    MissingFile(PathBuf),
    #[error("could not create package: {0}")]
    Archive(String),
}

pub fn build_vplugin(source: &Path, destination: &Path) -> Result<BuiltPackage, BuildError> {
    let manifest_path = source.join("manifest.json");
    let manifest_contents =
        fs::read(&manifest_path).map_err(|source_error| io_error(&manifest_path, source_error))?;
    let manifest: PluginManifest = serde_json::from_slice(&manifest_contents)
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    manifest
        .validate()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    require_manifest_files(source, &manifest)?;

    let mut paths = Vec::new();
    collect_paths(source, source, &mut paths)?;
    paths.sort();

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source_error| io_error(parent, source_error))?;
    }
    let file =
        File::create(destination).map_err(|source_error| io_error(destination, source_error))?;
    let encoder = GzBuilder::new().mtime(0).write(file, Compression::best());
    let mut archive = tar::Builder::new(encoder);
    archive.mode(tar::HeaderMode::Deterministic);
    for relative in paths {
        let path = source.join(&relative);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source_error| io_error(&path, source_error))?;
        if metadata.is_dir() {
            continue;
        }
        let mut input = File::open(&path).map_err(|source_error| io_error(&path, source_error))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(metadata.len());
        header.set_mode(if executable_path(&manifest) == Some(relative.as_path()) {
            0o755
        } else {
            0o644
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append_data(&mut header, &relative, &mut input)
            .map_err(|error| BuildError::Archive(error.to_string()))?;
    }
    let encoder = archive
        .into_inner()
        .map_err(|error| BuildError::Archive(error.to_string()))?;
    encoder
        .finish()
        .map_err(|source_error| io_error(destination, source_error))?;

    let sha256 = hash_file(destination)?;
    Ok(BuiltPackage {
        id: manifest.id,
        version: manifest.version,
        sha256,
    })
}

fn collect_paths(root: &Path, current: &Path, paths: &mut Vec<PathBuf>) -> Result<(), BuildError> {
    let entries = fs::read_dir(current).map_err(|source| io_error(current, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error(current, source))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BuildError::UnsupportedPath(path.clone()))?
            .to_path_buf();
        validate_source_path(&relative)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| io_error(&path, source))?;
        if metadata.is_dir() {
            collect_paths(root, &path, paths)?;
        } else if metadata.is_file() {
            paths.push(relative);
        } else {
            return Err(BuildError::UnsupportedType(relative));
        }
    }
    Ok(())
}

fn validate_source_path(path: &Path) -> Result<(), BuildError> {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(BuildError::UnsupportedPath(path.to_path_buf()));
    }
    let mut components = path.components();
    let first = components.next().and_then(|component| match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    });
    let allowed = match first {
        Some("manifest.json") => components.next().is_none(),
        Some("bin" | "qml" | "licenses") => true,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(BuildError::UnsupportedPath(path.to_path_buf()))
    }
}

fn require_manifest_files(root: &Path, manifest: &PluginManifest) -> Result<(), BuildError> {
    if let Runtime::NativeService { executable, .. } = &manifest.runtime {
        require_file(root, executable)?;
    }
    for path in [&manifest.ui.settings_page, &manifest.ui.dashboard_component]
        .into_iter()
        .flatten()
    {
        require_file(root, path)?;
    }
    Ok(())
}

fn require_file(root: &Path, relative: &str) -> Result<(), BuildError> {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            BuildError::MissingFile(PathBuf::from(relative))
        } else {
            io_error(&path, error)
        }
    })?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(BuildError::UnsupportedType(PathBuf::from(relative)))
    }
}

fn executable_path(manifest: &PluginManifest) -> Option<&Path> {
    match &manifest.runtime {
        Runtime::NativeService { executable, .. } => Some(Path::new(executable)),
        Runtime::QmlOnly => None,
    }
}

fn hash_file(path: &Path) -> Result<String, BuildError> {
    let mut file = File::open(path).map_err(|source| io_error(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn io_error(path: impl Into<PathBuf>, source: io::Error) -> BuildError {
    BuildError::Io {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use plugin_manager_core::{LocalRegistry, PackageExpectation};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn builds_a_deterministic_installable_package() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("qml")).unwrap();
        fs::create_dir_all(source.join("licenses")).unwrap();
        fs::write(
            source.join("manifest.json"),
            r#"{
                "schema": 1,
                "id": "demo",
                "name": "Demo",
                "version": "1.0.0",
                "runtime": { "kind": "qml-only" },
                "settings": { "enabled_path": "/Settings/Plugins/demo/Enabled" },
                "ui": { "settings_page": "qml/PageDemo.qml" }
            }"#,
        )
        .unwrap();
        fs::write(source.join("qml/PageDemo.qml"), "Item {}").unwrap();
        fs::write(source.join("licenses/LICENSE.txt"), "Demo license").unwrap();
        let first = temp.path().join("first.vplugin");
        let second = temp.path().join("second.vplugin");
        let built = build_vplugin(&source, &first).unwrap();
        let rebuilt = build_vplugin(&source, &second).unwrap();
        assert_eq!(built.sha256, rebuilt.sha256);

        let registry = LocalRegistry::new(temp.path().join("state"));
        registry
            .install_vplugin(
                &first,
                &PackageExpectation {
                    id: built.id,
                    version: built.version,
                    sha256: built.sha256,
                },
            )
            .unwrap();
        assert!(registry.load().unwrap().plugins.contains_key("demo"));
        let installed = &registry.load().unwrap().plugins["demo"];
        assert_eq!(
            fs::read_to_string(
                registry
                    .root()
                    .join(&installed.install_path)
                    .join("licenses/LICENSE.txt")
            )
            .unwrap(),
            "Demo license"
        );
    }

    #[test]
    fn rejects_extra_top_level_files() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("qml")).unwrap();
        fs::write(
            temp.path().join("manifest.json"),
            r#"{
                "schema": 1,
                "id": "demo",
                "name": "Demo",
                "version": "1.0.0",
                "runtime": { "kind": "qml-only" },
                "settings": { "enabled_path": "/Settings/Plugins/demo/Enabled" },
                "ui": { "settings_page": "qml/PageDemo.qml" }
            }"#,
        )
        .unwrap();
        fs::write(temp.path().join("qml/PageDemo.qml"), "Item {}").unwrap();
        fs::write(temp.path().join("extra.txt"), "no").unwrap();
        let error = build_vplugin(temp.path(), &temp.path().join("out.vplugin")).unwrap_err();
        assert!(matches!(error, BuildError::UnsupportedPath(_)));
    }
}
