use std::{env, fs, path::Path, process::ExitCode};

use plugin_manager_core::{
    Catalog, InstallOutcome, LocalRegistry, PackageExpectation, PluginManifest,
};

const USAGE: &str = "用法:
  venus-plugin-manager serve
  venus-plugin-manager version
  venus-plugin-manager install-manager
  venus-plugin-manager pack-vplugin <source-dir> <output.vplugin>
  venus-plugin-manager generate-signing-key <private-key-path>
  venus-plugin-manager sign-catalog-entry <private-key-path> <id> <version> <sha256>
  venus-plugin-manager validate-manifest <manifest.json>
  venus-plugin-manager validate-catalog <plugins.json>
  venus-plugin-manager validate-manager-release <release.json>
  venus-plugin-manager registry-init <state-root>
  venus-plugin-manager registry-list <state-root>
  venus-plugin-manager install-vplugin <state-root> <package.vplugin> <id> <version> <sha256>
  venus-plugin-manager uninstall <state-root> <id> --yes";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.as_slice() {
        [command] if command == "serve" => venus_plugin_manager::service::run(
            venus_plugin_manager::service::ServiceConfig::from_env(),
        )
        .map_err(|error| error.to_string()),
        [command] if command == "version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command] if command == "install-manager" => {
            venus_plugin_manager::installer::install(
                venus_plugin_manager::installer::InstallConfig::device(),
            )
            .map_err(|error| error.to_string())?;
            println!("Plugin Manager 已安装，入口位于 Settings > Plugins");
            Ok(())
        }
        [command, expected_version] if command == "apply-manager-update" => {
            apply_manager_update(expected_version)
        }
        [command, source, output] if command == "pack-vplugin" => {
            let package = venus_plugin_manager::package_builder::build_vplugin(
                Path::new(source),
                Path::new(output),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "built {} {}: {}",
                package.id, package.version, package.sha256
            );
            Ok(())
        }
        [command, path] if command == "generate-signing-key" => {
            let public = venus_plugin_manager::signing::generate_signing_key(Path::new(path))
                .map_err(|error| error.to_string())?;
            println!(
                "key_id={}\npublic_key={public}",
                venus_plugin_manager::signing::RELEASE_KEY_ID
            );
            Ok(())
        }
        [command, path, id, version, sha256] if command == "sign-catalog-entry" => {
            let signature = venus_plugin_manager::signing::sign_catalog_entry(
                Path::new(path),
                id,
                version,
                sha256,
            )
            .map_err(|error| error.to_string())?;
            println!("{}", signature.ed25519);
            Ok(())
        }
        [command, path] if command == "validate-manifest" => validate_manifest(Path::new(path)),
        [command, path] if command == "validate-catalog" => validate_catalog(Path::new(path)),
        [command, path] if command == "validate-manager-release" => {
            validate_manager_release(Path::new(path))
        }
        [command, root] if command == "registry-init" => registry_init(Path::new(root)),
        [command, root] if command == "registry-list" => registry_list(Path::new(root)),
        [command, root, package, id, version, sha256] if command == "install-vplugin" => {
            install_vplugin(
                Path::new(root),
                Path::new(package),
                PackageExpectation {
                    id: id.clone(),
                    version: version.clone(),
                    sha256: sha256.clone(),
                },
            )
        }
        [command, root, id, confirmation] if command == "uninstall" && confirmation == "--yes" => {
            uninstall(Path::new(root), id)
        }
        _ => Err(USAGE.into()),
    }
}

fn apply_manager_update(expected_version: &str) -> Result<(), String> {
    if expected_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "update expected version {expected_version}, binary is {}",
            env!("CARGO_PKG_VERSION")
        ));
    }
    let config = venus_plugin_manager::installer::InstallConfig::device();
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let download_root = env::var_os("VENUS_PLUGIN_MANAGER_DOWNLOAD_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(venus_plugin_manager::service::DEFAULT_DOWNLOAD_ROOT)
        });
    if executable.parent() != Some(download_root.as_path()) {
        return Err("manager update must run from the managed downloads directory".into());
    }
    venus_plugin_manager::installer::install(config).map_err(|error| error.to_string())?;
    fs::remove_file(&executable).map_err(|error| format!("{}: {error}", executable.display()))?;
    Ok(())
}

fn validate_manifest(path: &Path) -> Result<(), String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let manifest: PluginManifest =
        serde_json::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()))?;
    manifest
        .validate()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    println!("valid manifest: {} {}", manifest.id, manifest.version);
    Ok(())
}

fn validate_catalog(path: &Path) -> Result<(), String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let catalog: Catalog =
        serde_json::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()))?;
    catalog
        .validate()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let verifier = venus_plugin_manager::signing::CatalogVerifier::release()
        .map_err(|error| error.to_string())?;
    for entry in &catalog.plugins {
        verifier
            .verify(entry)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    println!("valid catalog: {} plugins", catalog.plugins.len());
    Ok(())
}

fn validate_manager_release(path: &Path) -> Result<(), String> {
    let contents = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let release = venus_plugin_manager::update::validate_release(&contents)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if release.version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "{}: release version {} does not match manager {}",
            path.display(),
            release.version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    println!("valid manager release: {}", release.version);
    Ok(())
}

fn registry_init(root: &Path) -> Result<(), String> {
    let registry = LocalRegistry::new(root);
    let state = registry.initialize().map_err(|error| error.to_string())?;
    println!(
        "本地插件 Registry 已初始化：{}（{} 个插件）",
        root.display(),
        state.plugins.len()
    );
    Ok(())
}

fn registry_list(root: &Path) -> Result<(), String> {
    let registry = LocalRegistry::new(root)
        .load()
        .map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(&registry).map_err(|error| error.to_string())?;
    println!("{json}");
    Ok(())
}

fn install_vplugin(
    root: &Path,
    package: &Path,
    expectation: PackageExpectation,
) -> Result<(), String> {
    let outcome = LocalRegistry::new(root)
        .install_vplugin(package, &expectation)
        .map_err(|error| error.to_string())?;
    let action = match outcome {
        InstallOutcome::Installed => "已安装",
        InstallOutcome::Updated => "已更新",
        InstallOutcome::Unchanged => "无需变更",
    };
    println!("{} {} {}", expectation.id, expectation.version, action);
    Ok(())
}

fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    LocalRegistry::new(root)
        .uninstall(id)
        .map_err(|error| error.to_string())?;
    println!("{id} 已从本地 Registry 和插件目录卸载");
    Ok(())
}
