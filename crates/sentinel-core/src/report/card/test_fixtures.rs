//! The three `card.json` fixtures every card test starts from, shaped per
//! finding type.

use crate::detect::FindingType;
use serde_json::{Value, json};

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("card fixture is JSON")
}

fn type_name(finding_type: FindingType) -> String {
    serde_json::to_value(finding_type)
        .expect("finding type serializes")
        .as_str()
        .expect("finding type is a string")
        .to_owned()
}

fn is_avoidable(name: &str) -> bool {
    name.starts_with("n_plus_one_") || name.starts_with("redundant_")
}

/// The story fixture for `finding_type`: an avoidable type keeps its
/// volume, CO2 and before-fix period; a threshold type cites one trace
/// (`cited_ms` for `slow_*`, `cited_ops` otherwise) with nothing else.
pub(crate) fn story_input(finding_type: FindingType) -> Value {
    let name = type_name(finding_type);
    let mut input = fixture(include_str!(
        "../../../../../tests/fixtures/card/story.json"
    ));
    input["story"]["finding_type"] = json!(name);
    if !is_avoidable(&name) {
        input["figures"] = if name.starts_with("slow_") {
            json!({"cited_ms": 2349})
        } else {
            json!({"cited_ops": 64})
        };
        input
            .as_object_mut()
            .expect("fixture is an object")
            .remove("before_fix_period");
    }
    input
}

/// [`story_input`] plus a threshold and the before-fix period it names:
/// August 2026, the month of `observed_at`.
pub(crate) fn story_input_with_threshold(finding_type: FindingType) -> Value {
    let name = type_name(finding_type.clone());
    assert!(!is_avoidable(&name), "{name} takes no threshold");
    let mut input = story_input(finding_type);
    if name.starts_with("slow_") {
        input["figures"]["threshold_ms"] = json!(500);
    } else {
        input["figures"]["threshold_ops"] = json!(50);
    }
    input["before_fix_period"] = json!({"from": "2026-08-01", "to": "2026-08-31"});
    input
}

pub(crate) fn curve_input() -> Value {
    fixture(include_str!(
        "../../../../../tests/fixtures/card/curve.json"
    ))
}

pub(crate) fn collective_input() -> Value {
    fixture(include_str!(
        "../../../../../tests/fixtures/card/collective.json"
    ))
}
