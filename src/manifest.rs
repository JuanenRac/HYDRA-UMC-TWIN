// =============================================================================
// HYDRA-UMC-TWIN - src/manifest.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! Real, defensive reading of a sibling repo's own `hydra-umc.project.json`.
//!
//! `hydra-umc.project.json` is already the single source of truth the
//! whole ecosystem's dashboard/updater trust - reading it back here rather
//! than hand-maintaining a
//! second list means this hub can never drift from a child's real,
//! current maturity.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "hydra-umc.project.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ChildManifest {
    pub name: String,
    pub version: String,
    pub maturity: String,
    pub role: String,
}

/// Why a child's manifest could not be used. A checkout that is not there is an
/// honest, expected outcome; a manifest that is there but cannot be read, parsed or
/// trusted is a different one and is reported as what it is, not as "not found".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// No checkout, or no manifest file in it.
    NotFound,
    /// The file is there and could not be read (permissions, not text).
    Unreadable(String),
    /// The file is there and is not a manifest: malformed JSON, a required field missing or of the wrong type,
    /// or a `schema_version` this Twin does not know.
    Unparseable(String),
    /// The file is a manifest, but not the one of this directory: another project's name or another ecosystem.
    WrongIdentity(String),
}

impl ManifestError {
    /// The sentence a report prints; `None` for a plain missing checkout.
    pub fn problem(&self) -> Option<String> {
        match self {
            ManifestError::NotFound => None,
            ManifestError::Unreadable(why) => Some(format!("manifest cannot be read: {why}")),
            ManifestError::Unparseable(why) => Some(format!("manifest is not usable: {why}")),
            ManifestError::WrongIdentity(why) => {
                Some(format!("manifest is not this project's: {why}"))
            }
        }
    }
}

/// The `schema_version` major this Twin understands.
const SUPPORTED_SCHEMA_MAJOR: &str = "1";
/// The ecosystem every manifest of the family names.
const ECOSYSTEM: &str = "HYDRA-UMC";

/// Reads `<repo_path>/hydra-umc.project.json` and checks it belongs there: its `name` is
/// `expected_name` (the directory it was read from), its `ecosystem` - when it has one - is the
/// family's, and its `schema_version` - when it has one - has the major this Twin knows.
pub fn read_child_manifest_checked(
    repo_path: &Path,
    expected_name: &str,
) -> Result<ChildManifest, ManifestError> {
    let text = match fs::read_to_string(repo_path.join(MANIFEST_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ManifestError::NotFound)
        }
        // a directory in place of the file, or a path whose parent is a file, is "nothing there" too
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotADirectory | std::io::ErrorKind::IsADirectory
            ) =>
        {
            return Err(ManifestError::NotFound)
        }
        Err(error) => return Err(ManifestError::Unreadable(error.to_string())),
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| ManifestError::Unparseable(error.to_string()))?;
    let manifest: ChildManifest = serde_json::from_value(value.clone())
        .map_err(|error| ManifestError::Unparseable(error.to_string()))?;
    if manifest.name != expected_name {
        return Err(ManifestError::WrongIdentity(format!(
            "it names {:?} but it was read from the directory of {:?}",
            manifest.name, expected_name
        )));
    }
    if let Some(ecosystem) = value.get("ecosystem").and_then(|v| v.as_str()) {
        if ecosystem != ECOSYSTEM {
            return Err(ManifestError::WrongIdentity(format!(
                "its ecosystem is {ecosystem:?}, not {ECOSYSTEM:?}"
            )));
        }
    }
    if let Some(schema) = value.get("schema_version") {
        let major = schema.as_str().and_then(|s| s.split('.').next());
        if major != Some(SUPPORTED_SCHEMA_MAJOR) {
            return Err(ManifestError::Unparseable(format!(
                "schema_version {schema} is not one this Twin knows (major {SUPPORTED_SCHEMA_MAJOR})"
            )));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_manifest(dir: &Path, content: &str) {
        let mut f = fs::File::create(dir.join(MANIFEST_FILE)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn reads_a_valid_manifest() {
        let tmp = tempdir();
        write_manifest(
            tmp.path(),
            r#"{"name": "HYDRA-UMC-PHYSICS-REPLICA", "version": "0.0.2", "maturity": "functional", "role": "library"}"#,
        );
        let m = read_child_manifest_checked(tmp.path(), "HYDRA-UMC-PHYSICS-REPLICA").unwrap();
        assert_eq!(m.name, "HYDRA-UMC-PHYSICS-REPLICA");
        assert_eq!(m.version, "0.0.2");
        assert_eq!(m.maturity, "functional");
        assert_eq!(m.role, "library");
    }

    #[test]
    fn missing_checkout_returns_none() {
        let never_created = std::env::temp_dir().join("hydra-umc-twin-does-not-exist");
        assert_eq!(
            read_child_manifest_checked(&never_created, "X"),
            Err(ManifestError::NotFound)
        );
    }

    #[test]
    fn missing_file_in_existing_dir_returns_none() {
        let tmp = tempdir();
        // Directory exists, but no manifest file was written into it.
        assert!(read_child_manifest_checked(tmp.path(), "X").is_err());
    }

    #[test]
    fn malformed_json_returns_none() {
        let tmp = tempdir();
        write_manifest(tmp.path(), "{ not valid json");
        assert!(read_child_manifest_checked(tmp.path(), "X").is_err());
    }

    #[test]
    fn missing_required_field_returns_none() {
        let tmp = tempdir();
        write_manifest(tmp.path(), r#"{"name": "X", "version": "0.0.1"}"#);
        assert!(read_child_manifest_checked(tmp.path(), "X").is_err());
    }

    fn named(name: &str, extra: &str) -> String {
        format!(
            r#"{{"name": "{name}", "version": "0.0.2", "maturity": "functional", "role": "library"{extra}}}"#
        )
    }

    #[test]
    fn checked_read_says_not_found_only_for_a_missing_checkout_or_file() {
        let tmp = tempdir();
        assert_eq!(
            read_child_manifest_checked(&tmp.path().join("nope"), "X"),
            Err(ManifestError::NotFound)
        );
        assert_eq!(
            read_child_manifest_checked(tmp.path(), "X"),
            Err(ManifestError::NotFound)
        );
        assert_eq!(ManifestError::NotFound.problem(), None);
    }

    #[test]
    fn checked_read_tells_malformed_json_from_a_missing_field() {
        let tmp = tempdir();
        write_manifest(tmp.path(), "{ not valid json");
        assert!(matches!(
            read_child_manifest_checked(tmp.path(), "X"),
            Err(ManifestError::Unparseable(_))
        ));
        write_manifest(tmp.path(), r#"{"name": "X", "version": "0.0.1"}"#);
        let error = read_child_manifest_checked(tmp.path(), "X").unwrap_err();
        assert!(
            matches!(&error, ManifestError::Unparseable(why) if why.contains("maturity")),
            "{error:?}"
        );
        assert!(error.problem().unwrap().contains("not usable"));
    }

    #[test]
    fn checked_read_refuses_another_projects_manifest_and_another_ecosystem() {
        let tmp = tempdir();
        write_manifest(tmp.path(), &named("HYDRA-UMC-SOMETHING-ELSE", ""));
        let error = read_child_manifest_checked(tmp.path(), "HYDRA-UMC-HIL-BRIDGE").unwrap_err();
        assert!(
            matches!(&error, ManifestError::WrongIdentity(why) if why.contains("SOMETHING-ELSE")),
            "{error:?}"
        );
        write_manifest(
            tmp.path(),
            &named("HYDRA-UMC-HIL-BRIDGE", r#", "ecosystem": "URTC""#),
        );
        assert!(matches!(
            read_child_manifest_checked(tmp.path(), "HYDRA-UMC-HIL-BRIDGE"),
            Err(ManifestError::WrongIdentity(_))
        ));
    }

    #[test]
    fn checked_read_checks_the_schema_major_when_there_is_one() {
        let tmp = tempdir();
        write_manifest(
            tmp.path(),
            &named(
                "X",
                r#", "schema_version": "1.0", "ecosystem": "HYDRA-UMC""#,
            ),
        );
        assert!(read_child_manifest_checked(tmp.path(), "X").is_ok());
        write_manifest(tmp.path(), &named("X", r#", "schema_version": "2.0""#));
        assert!(matches!(
            read_child_manifest_checked(tmp.path(), "X"),
            Err(ManifestError::Unparseable(_))
        ));
        write_manifest(tmp.path(), &named("X", r#", "schema_version": 1"#));
        assert!(matches!(
            read_child_manifest_checked(tmp.path(), "X"),
            Err(ManifestError::Unparseable(_))
        ));
        // a manifest without either field (older ones) is still accepted
        write_manifest(tmp.path(), &named("X", ""));
        assert!(read_child_manifest_checked(tmp.path(), "X").is_ok());
    }

    /// Minimal stdlib-only temp directory helper (no `tempfile` crate
    /// dependency for a handful of tests): a fresh directory under the
    /// OS temp dir, named for uniqueness with the current thread and a
    /// monotonically increasing counter, cleaned up on drop.
    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn tempdir() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("hydra-umc-twin-test-{}-{}", std::process::id(), n));
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}
