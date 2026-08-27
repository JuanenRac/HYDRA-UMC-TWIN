// =============================================================================
// HYDRA-UMC-TWIN - src/manifest.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! Real, defensive reading of a sibling repo's own `hydra-umc.project.json`.
//!
//! `hydra-umc.project.json` is already the single source of truth the
//! whole ecosystem's dashboard/updater trust (see `SONNET/BIBLIA
//! HYDRA-UMC`) - reading it back here rather than hand-maintaining a
//! second list means this hub can never drift from a child's real,
//! current maturity.

use std::fs;
use std::path::Path;

use serde::Deserialize;

pub const MANIFEST_FILE: &str = "hydra-umc.project.json";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ChildManifest {
    pub name: String,
    pub version: String,
    pub maturity: String,
    pub role: String,
}

/// Reads `<repo_path>/hydra-umc.project.json`. Returns `None` for every
/// real failure mode (missing checkout, missing file, malformed JSON,
/// missing required field) rather than panicking or propagating an error -
/// a missing sibling checkout is an honest, expected outcome here, not a
/// bug.
pub fn read_child_manifest(repo_path: &Path) -> Option<ChildManifest> {
    let text = fs::read_to_string(repo_path.join(MANIFEST_FILE)).ok()?;
    serde_json::from_str(&text).ok()
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
        let m = read_child_manifest(tmp.path()).unwrap();
        assert_eq!(m.name, "HYDRA-UMC-PHYSICS-REPLICA");
        assert_eq!(m.version, "0.0.2");
        assert_eq!(m.maturity, "functional");
        assert_eq!(m.role, "library");
    }

    #[test]
    fn missing_checkout_returns_none() {
        let never_created = std::env::temp_dir().join("hydra-umc-twin-does-not-exist");
        assert_eq!(read_child_manifest(&never_created), None);
    }

    #[test]
    fn missing_file_in_existing_dir_returns_none() {
        let tmp = tempdir();
        // Directory exists, but no manifest file was written into it.
        assert_eq!(read_child_manifest(tmp.path()), None);
    }

    #[test]
    fn malformed_json_returns_none() {
        let tmp = tempdir();
        write_manifest(tmp.path(), "{ not valid json");
        assert_eq!(read_child_manifest(tmp.path()), None);
    }

    #[test]
    fn missing_required_field_returns_none() {
        let tmp = tempdir();
        write_manifest(tmp.path(), r#"{"name": "X", "version": "0.0.1"}"#);
        assert_eq!(read_child_manifest(tmp.path()), None);
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
