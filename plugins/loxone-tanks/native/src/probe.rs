use serde_json::Value;
use thiserror::Error;

const SENSOR_TYPE: &str = "InfoOnlyAnalog";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TankKind {
    Fresh,
    Gray,
    Black,
}

impl TankKind {
    pub const ALL: [Self; 3] = [Self::Fresh, Self::Gray, Self::Black];

    pub const fn sensor_name(self) -> &'static str {
        match self {
            Self::Fresh => "fw tank",
            Self::Gray => "gw tank",
            Self::Black => "bw tank",
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Gray => "gray",
            Self::Black => "black",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TankSensorCandidate {
    pub tank: TankKind,
    pub name: String,
    pub control_uuid: String,
    pub state_uuid: String,
    pub format: String,
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("invalid Loxone Structure File: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Loxone Structure File does not contain controls")]
    MissingControls,
    #[error("required Loxone sensor {0} was not found")]
    MissingSensor(&'static str),
    #[error("more than one Loxone sensor matched {0}")]
    AmbiguousSensor(&'static str),
}

pub fn probe_structure(contents: &[u8]) -> Result<Vec<TankSensorCandidate>, ProbeError> {
    let structure: Value = serde_json::from_slice(contents)?;
    let controls = structure
        .get("controls")
        .and_then(Value::as_object)
        .ok_or(ProbeError::MissingControls)?;
    let mut candidates = Vec::new();

    for (control_uuid, control) in controls {
        let Some(name) = control.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(tank) = TankKind::ALL
            .into_iter()
            .find(|tank| name.trim().eq_ignore_ascii_case(tank.sensor_name()))
        else {
            continue;
        };
        if control.get("type").and_then(Value::as_str) != Some(SENSOR_TYPE) {
            continue;
        }
        let Some(state_uuid) = control
            .get("states")
            .and_then(Value::as_object)
            .and_then(|states| states.get("value"))
            .and_then(Value::as_str)
            .filter(|uuid| valid_uuid(uuid))
        else {
            continue;
        };
        if !valid_uuid(control_uuid) {
            continue;
        }
        candidates.push(TankSensorCandidate {
            tank,
            name: name.trim().to_owned(),
            control_uuid: control_uuid.clone(),
            state_uuid: state_uuid.to_owned(),
            format: control
                .get("details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("format"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }

    candidates.sort_by(|left, right| {
        left.tank
            .cmp(&right.tank)
            .then_with(|| left.control_uuid.cmp(&right.control_uuid))
    });
    Ok(candidates)
}

pub fn require_unique_tanks(
    candidates: &[TankSensorCandidate],
) -> Result<Vec<TankSensorCandidate>, ProbeError> {
    TankKind::ALL
        .into_iter()
        .map(|tank| {
            let mut matches = candidates.iter().filter(|candidate| candidate.tank == tank);
            let candidate = matches
                .next()
                .ok_or(ProbeError::MissingSensor(tank.sensor_name()))?;
            if matches.next().is_some() {
                return Err(ProbeError::AmbiguousSensor(tank.sensor_name()));
            }
            Ok(candidate.clone())
        })
        .collect()
}

fn valid_uuid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const FW_CONTROL: &str = "11111111-1111-1111-1111111111111111";
    const GW_CONTROL: &str = "22222222-2222-2222-2222222222222222";
    const BW_CONTROL: &str = "33333333-3333-3333-3333333333333333";

    #[test]
    fn only_fixed_read_only_tank_sensors_are_returned() {
        let structure = format!(
            r#"{{
                "controls": {{
                    "{FW_CONTROL}": {{
                        "name": " FW TANK ",
                        "type": "InfoOnlyAnalog",
                        "states": {{"value": "aaaaaaaa-aaaa-aaaa-aaaaaaaaaaaaaaaa"}},
                        "details": {{"format": "%.0f%%"}}
                    }},
                    "{GW_CONTROL}": {{
                        "name": "gw tank",
                        "type": "InfoOnlyAnalog",
                        "states": {{"value": "bbbbbbbb-bbbb-bbbb-bbbbbbbbbbbbbbbb"}}
                    }},
                    "{BW_CONTROL}": {{
                        "name": "bw tank",
                        "type": "InfoOnlyAnalog",
                        "states": {{"value": "cccccccc-cccc-cccc-cccccccccccccccc"}}
                    }},
                    "44444444-4444-4444-4444444444444444": {{
                        "name": "fw tank",
                        "type": "Switch",
                        "states": {{"active": "dddddddd-dddd-dddd-dddddddddddddddd"}}
                    }},
                    "55555555-5555-5555-5555555555555555": {{
                        "name": "engine temperature",
                        "type": "InfoOnlyAnalog",
                        "states": {{"value": "eeeeeeee-eeee-eeee-eeeeeeeeeeeeeeee"}}
                    }},
                    "66666666-6666-6666-6666666666666666": {{
                        "name": "fw tank 2",
                        "type": "InfoOnlyAnalog",
                        "states": {{"value": "ffffffff-ffff-ffff-ffffffffffffffff"}}
                    }}
                }}
            }}"#
        );

        let candidates = probe_structure(structure.as_bytes()).unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].tank, TankKind::Fresh);
        assert_eq!(candidates[0].name, "FW TANK");
        assert_eq!(candidates[1].tank, TankKind::Gray);
        assert_eq!(candidates[2].tank, TankKind::Black);
    }

    #[test]
    fn duplicate_exact_names_remain_explicit_choices() {
        let structure = format!(
            r#"{{"controls": {{
                "{FW_CONTROL}": {{
                    "name": "fw tank",
                    "type": "InfoOnlyAnalog",
                    "states": {{"value": "aaaaaaaa-aaaa-aaaa-aaaaaaaaaaaaaaaa"}}
                }},
                "{GW_CONTROL}": {{
                    "name": "FW TANK",
                    "type": "InfoOnlyAnalog",
                    "states": {{"value": "bbbbbbbb-bbbb-bbbb-bbbbbbbbbbbbbbbb"}}
                }}
            }}}}"#
        );
        let candidates = probe_structure(structure.as_bytes()).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates
            .iter()
            .all(|candidate| candidate.tank == TankKind::Fresh));
        assert!(matches!(
            require_unique_tanks(&candidates),
            Err(ProbeError::AmbiguousSensor("fw tank"))
        ));
    }

    #[test]
    fn all_three_unique_tanks_are_required() {
        let candidates = vec![TankSensorCandidate {
            tank: TankKind::Fresh,
            name: "FW Tank".to_owned(),
            control_uuid: FW_CONTROL.to_owned(),
            state_uuid: "aaaaaaaa-aaaa-aaaa-aaaaaaaaaaaaaaaa".to_owned(),
            format: "%.1f%%".to_owned(),
        }];
        assert!(matches!(
            require_unique_tanks(&candidates),
            Err(ProbeError::MissingSensor("gw tank"))
        ));
    }

    #[test]
    fn malformed_structure_is_rejected() {
        assert!(matches!(
            probe_structure(br#"{"msInfo": {}}"#),
            Err(ProbeError::MissingControls)
        ));
    }
}
