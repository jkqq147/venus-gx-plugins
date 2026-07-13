mod contract;
mod error;
mod lifecycle;
mod package;
mod registry;
mod transaction;

pub use contract::{
    Catalog, CatalogEntry, ContractError, DeviceListUi, PackageSignature, PackageSource,
    PluginManifest, PluginSettings, PluginUi, Runtime, CATALOG_SCHEMA_VERSION,
    MANIFEST_SCHEMA_VERSION,
};
pub use error::CoreError;
pub use lifecycle::{
    lifecycle_state, plan_reconciliation, DesiredPluginState, LifecycleState, ObservedPluginState,
    ReconcileAction, ServiceState,
};
pub use package::{validate_vplugin, PackageExpectation};
pub use registry::{InstalledPlugin, LocalRegistry, PluginRegistry, REGISTRY_SCHEMA_VERSION};
pub use transaction::InstallOutcome;
