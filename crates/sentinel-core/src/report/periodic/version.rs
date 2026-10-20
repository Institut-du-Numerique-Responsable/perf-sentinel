//! Which schema versions this engine reads. A newer report deserialises
//! without its new fields, so its hash would fail for the wrong reason:
//! the version is read from the raw JSON first.

use serde::Deserialize;

use super::SCHEMA_VERSION;

/// Every schema version this engine reads, oldest first, the current one last.
pub const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &[
    "perf-sentinel-report/v1.0",
    "perf-sentinel-report/v1.1",
    "perf-sentinel-report/v1.2",
    "perf-sentinel-report/v1.3",
    "perf-sentinel-report/v1.4",
    "perf-sentinel-report/v1.5",
    "perf-sentinel-report/v1.6",
    "perf-sentinel-report/v1.7",
    SCHEMA_VERSION,
];

#[derive(Debug, thiserror::Error)]
pub enum SchemaVersionError {
    /// No string `schema_version`, unparseable JSON included.
    #[error("the report carries no schema_version")]
    Missing,
    #[error("schema {0} is newer than this engine reads, upgrade perf-sentinel")]
    Unsupported(String),
    /// Not a `perf-sentinel-report/vMAJOR.MINOR` string at all.
    #[error("schema {0} is not a perf-sentinel report version")]
    Unknown(String),
}

#[derive(Deserialize)]
struct Probe {
    schema_version: Option<serde_json::Value>,
}

/// Read `schema_version` from the raw JSON and check this engine reads it.
///
/// # Errors
///
/// [`SchemaVersionError::Missing`] when the field is absent or not a string,
/// [`SchemaVersionError::Unsupported`] when this engine does not know it.
pub fn check_schema_version(raw: &[u8]) -> Result<(), SchemaVersionError> {
    let version = serde_json::from_slice::<Probe>(raw)
        .ok()
        .and_then(|probe| probe.schema_version)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(SchemaVersionError::Missing)?;
    if SUPPORTED_SCHEMA_VERSIONS.contains(&version.as_str()) {
        return Ok(());
    }
    // Only a well-formed version this engine does not list is a newer one.
    let well_formed = version
        .strip_prefix("perf-sentinel-report/v")
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(major, minor)| {
            [major, minor]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        });
    if well_formed {
        Err(SchemaVersionError::Unsupported(version))
    } else {
        Err(SchemaVersionError::Unknown(version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::periodic::SCHEMA_VERSION;

    #[test]
    fn accepts_every_published_version() {
        for minor in 0..=8 {
            let raw = format!(r#"{{"schema_version":"perf-sentinel-report/v1.{minor}"}}"#);
            assert!(check_schema_version(raw.as_bytes()).is_ok(), "v1.{minor}");
        }
    }

    #[test]
    fn refuses_a_newer_version() {
        let raw = br#"{"schema_version":"perf-sentinel-report/v1.9"}"#;
        assert!(matches!(
            check_schema_version(raw),
            Err(SchemaVersionError::Unsupported(v)) if v == "perf-sentinel-report/v1.9"
        ));
    }

    #[test]
    fn missing_version_is_missing() {
        assert!(matches!(
            check_schema_version(b"{}"),
            Err(SchemaVersionError::Missing)
        ));
        assert!(matches!(
            check_schema_version(b"not json"),
            Err(SchemaVersionError::Missing)
        ));
        assert!(matches!(
            check_schema_version(br#"{"schema_version":7}"#),
            Err(SchemaVersionError::Missing)
        ));
    }

    #[test]
    fn an_unknown_version_string_is_not_called_newer() {
        for raw in [
            "garbage",
            "v0.9",
            "perf-sentinel-report/v1",
            "perf-sentinel-report/vx.y",
        ] {
            let json = format!(r#"{{"schema_version":"{raw}"}}"#);
            assert!(
                matches!(
                    check_schema_version(json.as_bytes()),
                    Err(SchemaVersionError::Unknown(_))
                ),
                "{raw}"
            );
        }
        for raw in ["perf-sentinel-report/v1.9", "perf-sentinel-report/v2.0"] {
            let json = format!(r#"{{"schema_version":"{raw}"}}"#);
            assert!(
                matches!(
                    check_schema_version(json.as_bytes()),
                    Err(SchemaVersionError::Unsupported(_))
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn the_supported_list_has_no_gap() {
        let minors: Vec<String> = (0..SUPPORTED_SCHEMA_VERSIONS.len())
            .map(|minor| format!("perf-sentinel-report/v1.{minor}"))
            .collect();
        assert_eq!(minors, SUPPORTED_SCHEMA_VERSIONS);
    }

    #[test]
    fn the_current_version_is_supported() {
        assert_eq!(SUPPORTED_SCHEMA_VERSIONS.last(), Some(&SCHEMA_VERSION));
    }
}
