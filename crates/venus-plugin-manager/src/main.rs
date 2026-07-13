use std::{env, fs, path::Path, process::ExitCode};

use plugin_manager_core::{
    Catalog, InstallOutcome, LocalRegistry, PackageExpectation, PluginManifest,
};

const USAGE: &str = "用法:
  venus-plugin-manager validate-manifest <manifest.json>
  venus-plugin-manager validate-catalog <plugins.json>
  venus-plugin-manager registry-init <state-root>
  venus-plugin-manager registry-list <state-root>
  venus-plugin-manager install-vplugin <state-root> <package.vplugin> <id> <version> <sha256>
  venus-plugin-manager enable <state-root> <id>
  venus-plugin-manager disable <state-root> <id>
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
        [command, path] if command == "validate-manifest" => validate_manifest(Path::new(path)),
        [command, path] if command == "validate-catalog" => validate_catalog(Path::new(path)),
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
        [command, root, id] if command == "enable" => set_enabled(Path::new(root), id, true),
        [command, root, id] if command == "disable" => set_enabled(Path::new(root), id, false),
        [command, root, id, confirmation] if command == "uninstall" && confirmation == "--yes" => {
            uninstall(Path::new(root), id)
        }
        _ => Err(USAGE.into()),
    }
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
    println!("valid catalog: {} plugins", catalog.plugins.len());
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

fn set_enabled(root: &Path, id: &str, enabled: bool) -> Result<(), String> {
    LocalRegistry::new(root)
        .set_enabled(id, enabled)
        .map_err(|error| error.to_string())?;
    let state = if enabled { "启用" } else { "关闭" };
    println!("{id} 的本地期望状态已设为：{state}");
    Ok(())
}

fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    LocalRegistry::new(root)
        .uninstall(id)
        .map_err(|error| error.to_string())?;
    println!("{id} 已从本地 Registry 和插件目录卸载");
    Ok(())
}
