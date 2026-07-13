use serde::{Deserialize, Serialize};

use crate::{PluginManifest, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredPluginState {
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceState {
    NotApplicable,
    Stopped,
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPluginState {
    pub service: ServiceState,
    pub ui_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    StartService,
    StopService,
    ShowUi,
    HideUi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Disabled,
    Enabled,
    Converging,
    Degraded,
}

pub fn plan_reconciliation(
    manifest: &PluginManifest,
    desired: DesiredPluginState,
    observed: ObservedPluginState,
) -> Vec<ReconcileAction> {
    let mut actions = Vec::new();
    let should_show_ui = desired.enabled && !manifest.ui.is_empty();

    if desired.enabled {
        if matches!(manifest.runtime, Runtime::NativeService { .. })
            && observed.service != ServiceState::Running
        {
            actions.push(ReconcileAction::StartService);
        }
        if should_show_ui && !observed.ui_visible {
            actions.push(ReconcileAction::ShowUi);
        } else if !should_show_ui && observed.ui_visible {
            actions.push(ReconcileAction::HideUi);
        }
    } else {
        if observed.ui_visible {
            actions.push(ReconcileAction::HideUi);
        }
        if matches!(manifest.runtime, Runtime::NativeService { .. })
            && observed.service != ServiceState::Stopped
        {
            actions.push(ReconcileAction::StopService);
        }
    }

    actions
}

pub fn lifecycle_state(
    manifest: &PluginManifest,
    desired: DesiredPluginState,
    observed: ObservedPluginState,
) -> LifecycleState {
    if desired.enabled
        && matches!(manifest.runtime, Runtime::NativeService { .. })
        && observed.service == ServiceState::Failed
    {
        return LifecycleState::Degraded;
    }

    if plan_reconciliation(manifest, desired, observed).is_empty() {
        if desired.enabled {
            LifecycleState::Enabled
        } else {
            LifecycleState::Disabled
        }
    } else {
        LifecycleState::Converging
    }
}

#[cfg(test)]
mod tests {
    use crate::{PluginSettings, PluginUi, MANIFEST_SCHEMA_VERSION};

    use super::*;

    fn manifest(runtime: Runtime, ui: PluginUi) -> PluginManifest {
        PluginManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            id: "tpms".into(),
            name: "TPMS".into(),
            version: "0.1.0".into(),
            runtime,
            settings: PluginSettings {
                enabled_path: "/Settings/Plugins/tpms/Enabled".into(),
            },
            ui,
        }
    }

    fn native_manifest() -> PluginManifest {
        manifest(
            Runtime::NativeService {
                executable: "bin/venus-tpms-ble".into(),
            },
            PluginUi {
                settings_page: Some("qml/PageTpmsSettings.qml".into()),
                dashboard_component: None,
                device_list: None,
            },
        )
    }

    #[test]
    fn enables_service_before_showing_ui() {
        let actions = plan_reconciliation(
            &native_manifest(),
            DesiredPluginState { enabled: true },
            ObservedPluginState {
                service: ServiceState::Stopped,
                ui_visible: false,
            },
        );
        assert_eq!(
            actions,
            vec![ReconcileAction::StartService, ReconcileAction::ShowUi]
        );
    }

    #[test]
    fn hides_ui_before_stopping_service() {
        let actions = plan_reconciliation(
            &native_manifest(),
            DesiredPluginState { enabled: false },
            ObservedPluginState {
                service: ServiceState::Running,
                ui_visible: true,
            },
        );
        assert_eq!(
            actions,
            vec![ReconcileAction::HideUi, ReconcileAction::StopService]
        );
    }

    #[test]
    fn converged_state_has_no_actions() {
        let manifest = native_manifest();
        let desired = DesiredPluginState { enabled: true };
        let observed = ObservedPluginState {
            service: ServiceState::Running,
            ui_visible: true,
        };
        assert!(plan_reconciliation(&manifest, desired, observed).is_empty());
        assert_eq!(
            lifecycle_state(&manifest, desired, observed),
            LifecycleState::Enabled
        );
    }

    #[test]
    fn qml_only_plugin_never_requests_a_service_action() {
        let manifest = manifest(
            Runtime::QmlOnly,
            PluginUi {
                settings_page: Some("qml/Page.qml".into()),
                dashboard_component: None,
                device_list: None,
            },
        );
        let actions = plan_reconciliation(
            &manifest,
            DesiredPluginState { enabled: true },
            ObservedPluginState {
                service: ServiceState::NotApplicable,
                ui_visible: false,
            },
        );
        assert_eq!(actions, vec![ReconcileAction::ShowUi]);
    }

    #[test]
    fn headless_native_plugin_hides_unexpected_ui() {
        let manifest = manifest(
            Runtime::NativeService {
                executable: "bin/service".into(),
            },
            PluginUi::default(),
        );
        let actions = plan_reconciliation(
            &manifest,
            DesiredPluginState { enabled: true },
            ObservedPluginState {
                service: ServiceState::Running,
                ui_visible: true,
            },
        );
        assert_eq!(actions, vec![ReconcileAction::HideUi]);
    }

    #[test]
    fn failed_enabled_service_is_degraded() {
        let state = lifecycle_state(
            &native_manifest(),
            DesiredPluginState { enabled: true },
            ObservedPluginState {
                service: ServiceState::Failed,
                ui_visible: false,
            },
        );
        assert_eq!(state, LifecycleState::Degraded);
    }
}
