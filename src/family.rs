// =============================================================================
// HYDRA-UMC-TWIN - src/family.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! A real family-readiness check for the Digital Twin & Simulation
//! integration parent: is each real child checked out locally, and what
//! does it honestly report about itself.

use std::path::Path;

use crate::contract::{self, SyncReadiness};
use crate::manifest::{read_child_manifest, ChildManifest};

/// The 3 real children documented in this project's own README/manifest -
/// HYDRA-UMC-PHYSICS-REPLICA (the physics solver), HYDRA-UMC-HIL-BRIDGE
/// (hardware-in-the-loop command sync), HYDRA-UMC-SYNTHETIC-DATA-GEN
/// (rendered training datasets through this engine).
pub const EXPECTED_CHILDREN: [&str; 3] = [
    "HYDRA-UMC-PHYSICS-REPLICA",
    "HYDRA-UMC-HIL-BRIDGE",
    "HYDRA-UMC-SYNTHETIC-DATA-GEN",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ChildStatus {
    pub name: String,
    pub manifest: Option<ChildManifest>,
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
        .map(|&name| ChildStatus {
            name: name.to_string(),
            manifest: read_child_manifest(&workspace_root.join(name)),
        })
        .collect()
}

/// A child's sync readiness, combining whether it is checked out at all
/// with the real state-sync contract from `contract.rs`. A missing
/// checkout is its own, distinct outcome - there is no manifest to
/// assess a contract against.
#[derive(Debug, Clone, PartialEq)]
pub enum FamilySyncOutcome {
    Missing,
    Assessed(SyncReadiness),
}

impl FamilySyncOutcome {
    pub fn is_ready(&self) -> bool {
        matches!(self, FamilySyncOutcome::Assessed(r) if r.is_ready())
    }
}

#[derive(Debug, Clone, PartialEq)]
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
            let outcome = match &status.manifest {
                None => FamilySyncOutcome::Missing,
                Some(m) => FamilySyncOutcome::Assessed(contract::assess(m)),
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
