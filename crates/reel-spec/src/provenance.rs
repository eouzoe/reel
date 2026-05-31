//! Provenance — a content's lineage, recorded as a relationship, never as identity.
//!
//! Provenance answers *how content came to be* — never *what it is* (that is
//! the content [`crate::Hash`]). It is deliberately **not** a field of
//! [`crate::Block`]: because identical content deduplicates to a single Block,
//! one physical Block can be introduced by many different events and therefore
//! cannot own one lineage record. Lineage is a *relationship* — it attaches to
//! the **commit** that introduced the content (reel's analogue of a Nix
//! derivation / Git commit), forming the lineage/audit graph.
//!
//! At v0.1 this is a standalone record; the production `commit`
//! makes it a field of the commit-record. See `spec/spec.md` §5.2.
//!
//! # References
//!
//! - Dolstra, E. (2006). The Purely Functional Software Deployment Model.
//!   (Nix: a store object is addressed by *what it is*; a derivation records
//!   *how it was made*.)
//! - W3C PROV-O: `Entity wasGeneratedBy Activity wasAttributedTo Agent`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lineage of introduced content: which kernel produced it, when, and
/// (eventually) by whom.
///
/// Recorded on the introducing commit (the lineage/audit graph), **not** on
/// [`crate::Block`] — content identity is `BLAKE3(data)` alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The reel kernel version that produced the commit (semver string).
    pub kernel_version: String,

    /// UTC timestamp at which the content was introduced.
    pub created_at: DateTime<Utc>,

    /// Optional Ed25519 public key (32 raw bytes) of the signing party.
    ///
    /// Reserved for Portal Daemon integration (post-MVP); `None` in all
    /// v0.1 implementations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<[u8; 32]>,
}

impl Provenance {
    /// Construct a [`Provenance`] from the current kernel version and
    /// wall-clock time.
    ///
    /// - `kernel_version`: `env!("CARGO_PKG_VERSION")` at compile time.
    /// - `created_at`: `Utc::now()` at call time.
    /// - `signer`: `None` (reserved post-MVP).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Provenance;
    ///
    /// let p = Provenance::current();
    /// assert_eq!(p.kernel_version, env!("CARGO_PKG_VERSION"));
    /// assert!(p.signer.is_none());
    /// ```
    #[must_use]
    pub fn current() -> Self {
        Self {
            kernel_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at: Utc::now(),
            signer: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::de::from_reader as cbor_decode;
    use ciborium::ser::into_writer as cbor_enc;
    use std::error::Error;

    #[test]
    fn current_has_pkg_version_and_no_signer() {
        let p = Provenance::current();
        assert_eq!(p.kernel_version, env!("CARGO_PKG_VERSION"));
        assert!(p.signer.is_none(), "signer MUST be None in v0.1");
    }

    #[test]
    fn cbor_round_trip() -> Result<(), Box<dyn Error>> {
        use chrono::TimeZone as _;
        #[allow(
            clippy::unwrap_used,
            reason = "literal date 2026-05-22T00:00:00Z is infallible by construction"
        )]
        let created_at = Utc.with_ymd_and_hms(2026, 5, 22, 0, 0, 0).unwrap();
        let prov = Provenance { kernel_version: "0.1.0".to_owned(), created_at, signer: None };
        let mut buf = Vec::new();
        cbor_enc(&prov, &mut buf)?;
        let prov2: Provenance = cbor_decode(buf.as_slice())?;
        assert_eq!(prov, prov2, "CBOR round-trip preserves all fields");
        Ok(())
    }

    #[test]
    fn serde_omits_none_signer() -> Result<(), Box<dyn Error>> {
        let prov =
            Provenance { kernel_version: "0.1.0".to_owned(), created_at: Utc::now(), signer: None };
        let json = serde_json::to_string(&prov)?;
        assert!(!json.contains("signer"), "signer absent from JSON when None: {json}");
        Ok(())
    }
}
