// =============================================================================
// HYDRA-UMC-TWIN - src/contract.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! The real state-sync contract this Twin enforces before ever treating a
//! child as sync-ready: `family.rs`'s `check_family_status()` already
//! answers "is this child checked out, and what does it claim about
//! itself" - this module answers the next real question, "do I actually
//! trust that claim enough to sync state with it."
//!
//! Two real gates, both required:
//!   - Maturity must be at least `MIN_MATURITY` ("functional") - a
//!     `scaffolding` child has no real state worth syncing yet, only an
//!     entry point.
//!   - The child's reported major version must not exceed
//!     `MAX_COMPATIBLE_MAJOR` - the highest major version this Twin's
//!     sync logic has actually been verified against. A child that has
//!     bumped past that is an incompatible simulator version: its own
//!     state contract may have changed in a way this Twin does not yet
//!     know how to handle, so it is refused rather than synced against
//!     blindly.
//! A child that clears both gates gets a real `SyncSnapshot` - the
//! fixture representing exactly what would be exchanged, real and
//! testable independent of any actual network sync transport (which
//! doesn't exist yet - see `family.rs`'s own honesty note).

use crate::manifest::ChildManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaturityLevel {
    Scaffolding,
    Functional,
    Established,
}

impl MaturityLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "scaffolding" => Some(MaturityLevel::Scaffolding),
            "functional" => Some(MaturityLevel::Functional),
            "established" => Some(MaturityLevel::Established),
            _ => None,
        }
    }
}

/// The minimum maturity this Twin requires before syncing state with a
/// child - see the module docs for why `scaffolding` is excluded.
pub const MIN_MATURITY: MaturityLevel = MaturityLevel::Functional;

/// The highest child major version this Twin's sync contract has been
/// verified against. Every child in this ecosystem is still pre-1.0
/// (major 0), so this starts at 0 - bump deliberately, alongside a real
/// review of what changed, whenever a child's own major version moves.
pub const MAX_COMPATIBLE_MAJOR: u32 = 0;

/// The real fixture representing what state this Twin would exchange
/// with a child once it is sync-ready. Deliberately minimal today (no
/// live simulation tick data exists yet - see `family.rs`), but a real,
/// typed value rather than a loose string/JSON blob, so a future sync
/// transport has a concrete contract to serialize instead of inventing
/// one from scratch.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncSnapshot {
    pub child: String,
    pub version: String,
    pub maturity: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncReadiness {
    Ready(SyncSnapshot),
    ImmatureMaturity { reason: String },
    IncompatibleVersion { reason: String },
    UnparseableManifest { reason: String },
}

impl SyncReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, SyncReadiness::Ready(_))
    }
}

fn parse_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Assesses one child's manifest against the real sync contract above.
/// Maturity is checked before version compatibility - an immature child
/// is refused for that reason even if its version happens to parse as
/// compatible, so the rejection reason always names the real, most
/// fundamental gate that failed.
pub fn assess(manifest: &ChildManifest) -> SyncReadiness {
    let maturity = match MaturityLevel::parse(&manifest.maturity) {
        Some(m) => m,
        None => {
            return SyncReadiness::UnparseableManifest {
                reason: format!(
                    "{} reports unrecognized maturity {:?} - refusing to sync against an unknown contract",
                    manifest.name, manifest.maturity
                ),
            }
        }
    };
    if maturity < MIN_MATURITY {
        return SyncReadiness::ImmatureMaturity {
            reason: format!(
                "{} maturity is {:?}, below the minimum {:?} required for state sync",
                manifest.name, maturity, MIN_MATURITY
            ),
        };
    }

    let major = match parse_major(&manifest.version) {
        Some(m) => m,
        None => {
            return SyncReadiness::UnparseableManifest {
                reason: format!(
                    "{} reports an unparseable version {:?}",
                    manifest.name, manifest.version
                ),
            }
        }
    };
    if major > MAX_COMPATIBLE_MAJOR {
        return SyncReadiness::IncompatibleVersion {
            reason: format!(
                "{} reports major version {} - this Twin's sync contract is only verified up to major {} (incompatible simulator version)",
                manifest.name, major, MAX_COMPATIBLE_MAJOR
            ),
        };
    }

    SyncReadiness::Ready(SyncSnapshot {
        child: manifest.name.clone(),
        version: manifest.version.clone(),
        maturity: manifest.maturity.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, version: &str, maturity: &str) -> ChildManifest {
        ChildManifest {
            name: name.to_string(),
            version: version.to_string(),
            maturity: maturity.to_string(),
            role: "library".to_string(),
        }
    }

    #[test]
    fn functional_child_at_compatible_major_is_ready() {
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "0.0.5", "functional");
        let outcome = assess(&m);
        assert!(outcome.is_ready());
        assert_eq!(
            outcome,
            SyncReadiness::Ready(SyncSnapshot {
                child: "HYDRA-UMC-PHYSICS-REPLICA".to_string(),
                version: "0.0.5".to_string(),
                maturity: "functional".to_string(),
            })
        );
    }

    #[test]
    fn established_child_is_also_ready() {
        // Boundary: established is strictly above the minimum, not just
        // at it - must still be accepted.
        let m = manifest("HYDRA-UMC-HIL-BRIDGE", "0.1.2", "established");
        assert!(assess(&m).is_ready());
    }

    #[test]
    fn scaffolding_child_is_rejected_as_immature() {
        let m = manifest("HYDRA-UMC-SYNTHETIC-DATA-GEN", "0.0.1", "scaffolding");
        let outcome = assess(&m);
        assert!(!outcome.is_ready());
        match outcome {
            SyncReadiness::ImmatureMaturity { reason } => {
                assert!(reason.contains("HYDRA-UMC-SYNTHETIC-DATA-GEN"));
            }
            other => panic!("expected ImmatureMaturity, got {other:?}"),
        }
    }

    #[test]
    fn major_version_at_the_compatibility_ceiling_is_ready() {
        // Boundary: major == MAX_COMPATIBLE_MAJOR (0) counts as
        // compatible, not one past it.
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "0.9.9", "functional");
        assert!(assess(&m).is_ready());
    }

    #[test]
    fn major_version_past_the_compatibility_ceiling_is_rejected() {
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "1.0.0", "functional");
        let outcome = assess(&m);
        assert!(!outcome.is_ready());
        match outcome {
            SyncReadiness::IncompatibleVersion { reason } => {
                assert!(reason.contains("major version 1"));
            }
            other => panic!("expected IncompatibleVersion, got {other:?}"),
        }
    }

    #[test]
    fn immaturity_is_checked_before_version_compatibility() {
        // Both gates would fail here - the reported reason must be the
        // maturity one, since that is checked first.
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "3.0.0", "scaffolding");
        match assess(&m) {
            SyncReadiness::ImmatureMaturity { .. } => {}
            other => panic!("expected ImmatureMaturity to win, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_maturity_string_is_rejected_honestly() {
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "0.0.1", "andamiaje");
        match assess(&m) {
            SyncReadiness::UnparseableManifest { reason } => {
                assert!(reason.contains("andamiaje"));
            }
            other => panic!("expected UnparseableManifest, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_version_string_is_rejected_honestly() {
        let m = manifest("HYDRA-UMC-PHYSICS-REPLICA", "not-a-version", "functional");
        match assess(&m) {
            SyncReadiness::UnparseableManifest { reason } => {
                assert!(reason.contains("not-a-version"));
            }
            other => panic!("expected UnparseableManifest, got {other:?}"),
        }
    }

    #[test]
    fn maturity_level_ordering_is_scaffolding_lowest_established_highest() {
        assert!(MaturityLevel::Scaffolding < MaturityLevel::Functional);
        assert!(MaturityLevel::Functional < MaturityLevel::Established);
    }
}
