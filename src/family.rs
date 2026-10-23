// =============================================================================
// HYDRA-UMC-TWIN - src/family.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! A real family-readiness check for the Digital Twin & Simulation
//! integration parent: is each real child checked out locally, and what
//! does it honestly report about itself.

use std::path::Path;

use serde::Serialize;

use crate::contract::{self, SyncReadiness};
use crate::manifest::{read_child_manifest_checked, ChildManifest, ManifestError};

/// The 3 real children documented in this project's own README/manifest -
/// HYDRA-UMC-PHYSICS-REPLICA (the physics solver), HYDRA-UMC-HIL-BRIDGE
/// (hardware-in-the-loop command sync), HYDRA-UMC-SYNTHETIC-DATA-GEN
/// (rendered training datasets through this engine).
pub const EXPECTED_CHILDREN: [&str; 3] = [
    "HYDRA-UMC-PHYSICS-REPLICA",
    "HYDRA-UMC-HIL-BRIDGE",
    "HYDRA-UMC-SYNTHETIC-DATA-GEN",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildStatus {
    pub name: String,
    pub manifest: Option<ChildManifest>,
    /// Why there is no usable manifest when the checkout IS there: the file cannot be read, is not a
    /// manifest, or belongs to another project. `None` when the child is fine or simply not checked out.
    pub problem: Option<String>,
}

impl ChildStatus {
    pub fn is_present(&self) -> bool {
        self.manifest.is_some()
    }
}

/// Checks every expected child against a real local workspace, reading
/// each one's own manifest rather than a second hand-maintained list -
/// if a child's real maturity changes, this reflects it on the very next
/// run, honestly, with no separate list to fall out of sync.
pub fn check_family_status(workspace_root: &Path) -> Vec<ChildStatus> {
    EXPECTED_CHILDREN
        .iter()
        .map(|&name| {
            let (manifest, problem) =
                match read_child_manifest_checked(&workspace_root.join(name), name) {
                    Ok(manifest) => (Some(manifest), None),
                    Err(error) => (None, ManifestError::problem(&error)),
                };
            ChildStatus {
                name: name.to_string(),
                manifest,
                problem,
            }
        })
        .collect()
}

/// A child's sync readiness, combining whether it is checked out at all
/// with the real state-sync contract from `contract.rs`. A missing
/// checkout is its own, distinct outcome - there is no manifest to
/// assess a contract against.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FamilySyncOutcome {
    Missing,
    /// Checked out, but its manifest is unreadable, malformed or not its own: there is nothing to assess.
    Unusable {
        reason: String,
    },
    Assessed(SyncReadiness),
}

impl FamilySyncOutcome {
    pub fn is_ready(&self) -> bool {
        matches!(self, FamilySyncOutcome::Assessed(r) if r.is_ready())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FamilySyncStatus {
    pub name: String,
    pub outcome: FamilySyncOutcome,
}

/// The real, combined answer to "which children can this Twin actually
/// sync state with right now": every expected child, checked out or
/// not, run through the real `contract::assess()` gate when it is.
pub fn assess_family_sync(workspace_root: &Path) -> Vec<FamilySyncStatus> {
    check_family_status(workspace_root)
        .into_iter()
        .map(|status| {
            let outcome = match (&status.manifest, &status.problem) {
                (Some(m), _) => FamilySyncOutcome::Assessed(contract::assess(m)),
                (None, Some(reason)) => FamilySyncOutcome::Unusable {
                    reason: reason.clone(),
                },
                (None, None) => FamilySyncOutcome::Missing,
            };
            FamilySyncStatus {
                name: status.name,
                outcome,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

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
        let path = std::env::temp_dir().join(format!(
            "hydra-umc-twin-family-test-{}-{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn write_manifest(workspace: &Path, repo: &str, maturity: &str) {
        let dir = workspace.join(repo);
        fs::create_dir_all(&dir).unwrap();
        let mut f = fs::File::create(dir.join("hydra-umc.project.json")).unwrap();
        write!(
            f,
            r#"{{"name": "{repo}", "version": "0.0.1", "maturity": "{maturity}", "role": "library"}}"#
        )
        .unwrap();
    }

    fn write_raw(workspace: &Path, repo: &str, content: &str) {
        let dir = workspace.join(repo);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hydra-umc.project.json"), content).unwrap();
    }

    #[test]
    fn a_malformed_manifest_is_a_problem_not_a_missing_checkout() {
        let ws = tempdir();
        write_raw(ws.path(), "HYDRA-UMC-HIL-BRIDGE", "{ broken");
        let statuses = check_family_status(ws.path());
        let hil = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-HIL-BRIDGE")
            .unwrap();
        assert!(!hil.is_present());
        assert!(
            hil.problem.as_deref().unwrap().contains("not usable"),
            "{:?}",
            hil.problem
        );
        let missing = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-PHYSICS-REPLICA")
            .unwrap();
        assert_eq!(
            missing.problem, None,
            "a checkout that is not there has no problem to report"
        );
        let sync = assess_family_sync(ws.path());
        let outcome = &sync
            .iter()
            .find(|s| s.name == "HYDRA-UMC-HIL-BRIDGE")
            .unwrap()
            .outcome;
        assert!(
            matches!(outcome, FamilySyncOutcome::Unusable { .. }),
            "{outcome:?}"
        );
        assert!(!outcome.is_ready());
    }

    #[test]
    fn another_projects_manifest_in_a_directory_is_refused() {
        let ws = tempdir();
        // the manifest of one child copied into the directory of another
        write_raw(
            ws.path(),
            "HYDRA-UMC-HIL-BRIDGE",
            r#"{"name": "HYDRA-UMC-PHYSICS-REPLICA", "version": "0.0.1", "maturity": "functional", "role": "library"}"#,
        );
        let statuses = check_family_status(ws.path());
        let hil = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-HIL-BRIDGE")
            .unwrap();
        assert!(!hil.is_present());
        assert!(hil.problem.as_deref().unwrap().contains("PHYSICS-REPLICA"));
        assert!(!assess_family_sync(ws.path())
            .iter()
            .any(|s| s.outcome.is_ready()));
    }

    #[test]
    fn all_children_present() {
        let ws = tempdir();
        for name in EXPECTED_CHILDREN {
            write_manifest(ws.path(), name, "functional");
        }
        let statuses = check_family_status(ws.path());
        assert_eq!(statuses.len(), 3);
        assert!(statuses.iter().all(|s| s.is_present()));
    }

    #[test]
    fn some_children_missing() {
        let ws = tempdir();
        write_manifest(ws.path(), "HYDRA-UMC-PHYSICS-REPLICA", "functional");
        // HIL-BRIDGE and SYNTHETIC-DATA-GEN left unwritten - real "missing".
        let statuses = check_family_status(ws.path());
        let present: Vec<&str> = statuses
            .iter()
            .filter(|s| s.is_present())
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(present, vec!["HYDRA-UMC-PHYSICS-REPLICA"]);
    }

    #[test]
    fn none_present_on_empty_workspace() {
        let ws = tempdir();
        let statuses = check_family_status(ws.path());
        assert!(statuses.iter().all(|s| !s.is_present()));
    }

    #[test]
    fn reports_real_maturity_from_manifest() {
        let ws = tempdir();
        write_manifest(ws.path(), "HYDRA-UMC-HIL-BRIDGE", "scaffolding");
        let statuses = check_family_status(ws.path());
        let hil = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-HIL-BRIDGE")
            .unwrap();
        assert_eq!(hil.manifest.as_ref().unwrap().maturity, "scaffolding");
    }

    #[test]
    fn sync_outcome_is_missing_for_an_uncheckedout_child() {
        let ws = tempdir();
        // Nothing written for any child.
        let statuses = assess_family_sync(ws.path());
        assert!(statuses
            .iter()
            .all(|s| s.outcome == FamilySyncOutcome::Missing));
        assert!(statuses.iter().all(|s| !s.outcome.is_ready()));
    }

    #[test]
    fn sync_outcome_is_ready_for_a_functional_child_at_a_compatible_version() {
        let ws = tempdir();
        write_manifest(ws.path(), "HYDRA-UMC-PHYSICS-REPLICA", "functional");
        let statuses = assess_family_sync(ws.path());
        let replica = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-PHYSICS-REPLICA")
            .unwrap();
        assert!(replica.outcome.is_ready());
    }

    #[test]
    fn sync_outcome_rejects_a_scaffolding_child_via_the_real_contract() {
        let ws = tempdir();
        write_manifest(ws.path(), "HYDRA-UMC-SYNTHETIC-DATA-GEN", "scaffolding");
        let statuses = assess_family_sync(ws.path());
        let gen = statuses
            .iter()
            .find(|s| s.name == "HYDRA-UMC-SYNTHETIC-DATA-GEN")
            .unwrap();
        assert!(!gen.outcome.is_ready());
        match &gen.outcome {
            FamilySyncOutcome::Assessed(SyncReadiness::ImmatureMaturity { .. }) => {}
            other => panic!("expected Assessed(ImmatureMaturity), got {other:?}"),
        }
    }
}
