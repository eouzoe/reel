//! CBOR encode / decode helpers used by both `MemStore` and `RedbStore`.
//!
//! All on-disk and intra-process values in `reel-store` are encoded as
//! [`ciborium`]-flavoured CBOR (RFC 8949) per ADR-0017.
//!
//! Errors are normalised onto [`crate::StoreError::Codec`] so the call
//! sites do not have to take a transitive dependency on [`ciborium`].

use ciborium::de::from_reader;
use ciborium::ser::into_writer;
use serde::{Serialize, de::DeserializeOwned};

use crate::error::StoreError;

/// Encode `value` as CBOR.
///
/// # Errors
///
/// Returns [`StoreError::Codec`] if the serialiser fails — typically
/// because `T`'s `Serialize` impl returned an error.
#[allow(
    clippy::redundant_pub_crate,
    reason = "module is private; pub(crate) makes intent explicit and satisfies workspace `unreachable_pub` lint"
)]
pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    let mut buf = Vec::new();
    into_writer(value, &mut buf).map_err(|e| StoreError::Codec(e.to_string()))?;
    Ok(buf)
}

/// Decode `bytes` as CBOR into a `T`.
///
/// # Errors
///
/// Returns [`StoreError::Codec`] if the bytes are not valid CBOR for
/// `T` (truncated input, unexpected type, etc.).
#[allow(
    clippy::redundant_pub_crate,
    reason = "module is private; pub(crate) makes intent explicit and satisfies workspace `unreachable_pub` lint"
)]
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StoreError> {
    from_reader(bytes).map_err(|e| StoreError::Codec(e.to_string()))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are acceptable for test failures"
)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Probe {
        a: u32,
        b: String,
    }

    #[test]
    fn round_trip() -> Result<(), StoreError> {
        let p = Probe { a: 7, b: "reel".to_owned() };
        let buf = encode(&p)?;
        let q: Probe = decode(&buf)?;
        assert_eq!(p, q);
        Ok(())
    }

    #[test]
    fn decode_rejects_garbage() {
        let bad = [0xff_u8, 0xff, 0xff];
        let r: Result<Probe, _> = decode(&bad);
        assert!(r.is_err(), "garbage bytes must not decode to Probe");
    }
}
