//! BLAKE3-256 digest — Block identity constructor.
//!
//! [`struct@Hash`] wraps a raw 32-byte BLAKE3-256 digest and serves as the
//! identity constructor for [`crate::Block`].
//! See `spec/spec.md` §5.1 and ADR-0004.
//!
//! # References
//!
//! - ADR-0004
//! - BLAKE3 specification: <https://github.com/BLAKE3-team/BLAKE3>

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Write as FmtWrite;

/// BLAKE3-256 digest serving as Block identity.
///
/// `Hash` is a 32-byte newtype representing a BLAKE3-256 digest.
/// Two Blocks with identical content (data + provenance, per ADR-0004)
/// produce the same `Hash`; this is the foundation of content-addressed
/// storage in reel (I-001 Reachability).
///
/// `Hash` is `Copy`, `Eq`, `Ord`, and `Hash`-able, making it suitable
/// as a map key and for use in [`std::collections::BTreeMap`].
///
/// # Examples
///
/// ```rust
/// use reel_spec::Hash;
///
/// let raw: [u8; 32] = *blake3::hash(b"hello reel").as_bytes();
/// let h = Hash::from_bytes(raw);
/// assert_eq!(h.as_bytes().len(), 32);
///
/// let hex = h.to_hex();
/// let h2 = Hash::from_hex(&hex).expect("round-trip should succeed");
/// assert_eq!(h, h2);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Hash(
    /// Raw 32-byte BLAKE3-256 digest.
    #[serde(with = "hex_serde")]
    [u8; 32],
);

impl Hash {
    /// Constructs a `Hash` from a raw 32-byte array.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Hash;
    ///
    /// let raw: [u8; 32] = *blake3::hash(b"data").as_bytes();
    /// let h = Hash::from_bytes(raw);
    /// assert_eq!(h.as_bytes(), &raw);
    /// ```
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns a reference to the raw 32-byte digest.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Hash;
    ///
    /// let h = Hash::from_bytes([0u8; 32]);
    /// assert_eq!(h.as_bytes().len(), 32);
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the lowercase hexadecimal representation of the digest.
    ///
    /// The returned string is always exactly 64 characters.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Hash;
    ///
    /// let h = Hash::from_bytes([0u8; 32]);
    /// let hex = h.to_hex();
    /// assert_eq!(hex.len(), 64);
    /// assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    /// ```
    #[must_use]
    pub fn to_hex(&self) -> String {
        bytes_to_hex(&self.0)
    }

    /// Parses a 64-character lowercase hexadecimal string into a `Hash`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ReelError::Storage`] if `s` is not exactly 64 valid
    /// hex characters.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reel_spec::Hash;
    ///
    /// let h = Hash::from_bytes([0u8; 32]);
    /// let hex = h.to_hex();
    /// let h2 = Hash::from_hex(&hex).expect("valid hex");
    /// assert_eq!(h, h2);
    ///
    /// let err = Hash::from_hex("not-hex");
    /// assert!(err.is_err());
    /// ```
    pub fn from_hex(s: &str) -> Result<Self, crate::ReelError> {
        hex_to_bytes(s).map(Self).map_err(|e| crate::ReelError::Storage(e.to_string()))
    }
}

/// Encodes a 32-byte array as 64 lowercase hex characters.
fn bytes_to_hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        // `write!` on `String` is infallible — `fmt::Write` on `String`
        // never returns `Err`.  `.ok()` discards the `Result` explicitly.
        write!(out, "{b:02x}").ok();
    }
    out
}

/// Decodes a 64-character hex string to a 32-byte array.
fn hex_to_bytes(s: &str) -> Result<[u8; 32], HexError> {
    if s.len() != 64 {
        return Err(HexError::Length(s.len()));
    }
    let mut bytes = [0u8; 32];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let Some((&hi_byte, &lo_byte)) = pair.first().zip(pair.get(1)) else {
            return Err(HexError::Truncated);
        };
        let hi = hex_nibble(hi_byte)?;
        let lo = hex_nibble(lo_byte)?;
        // `i` is at most 31 (32-byte array, 64-char hex).
        if let Some(slot) = bytes.get_mut(i) {
            *slot = hi.wrapping_shl(4) | lo;
        }
    }
    Ok(bytes)
}

/// Decodes a single ASCII hex character to its numeric nibble value.
const fn hex_nibble(c: u8) -> Result<u8, HexError> {
    match c {
        b'0'..=b'9' => Ok(c.wrapping_sub(b'0')),
        b'a'..=b'f' => Ok(c.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Ok(c.wrapping_sub(b'A').wrapping_add(10)),
        _ => Err(HexError::Char(c)),
    }
}

/// Error returned when decoding a hex string fails.
#[derive(Debug, Clone, Copy)]
enum HexError {
    Length(usize),
    Char(u8),
    Truncated,
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(n) => write!(f, "Hash::from_hex: expected 64 hex chars, got {n}"),
            Self::Char(c) => write!(f, "invalid hex character: 0x{c:02x}"),
            Self::Truncated => write!(f, "Hash::from_hex: truncated chunk"),
        }
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.to_hex();
        let prefix = hex.get(..8).unwrap_or(&hex);
        write!(f, "Hash({prefix}…)")
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl From<[u8; 32]> for Hash {
    fn from(bytes: [u8; 32]) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<blake3::Hash> for Hash {
    fn from(h: blake3::Hash) -> Self {
        Self::from_bytes(*h.as_bytes())
    }
}

impl From<Hash> for [u8; 32] {
    fn from(h: Hash) -> Self {
        h.0
    }
}

/// Serde module: encodes `[u8; 32]` as a 64-character hex string.
mod hex_serde {
    use super::{HexError, bytes_to_hex, hex_nibble};
    use serde::de::Error as DeError;
    use serde::{Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8; 32], ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&bytes_to_hex(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<[u8; 32], D::Error> {
        // Owned `String` (not `&str`): ciborium implements `visit_string` not
        // `visit_str`, so borrowed deserialise of any type containing `Hash`
        // (Block/Ref/Delta) fails the CBOR codec path. This was a latent
        // defect surfaced cumulatively by/ audits + rework
        // root-cause. Lead arbitration patch 2026-05-22; see
        // the internal design notes
        let s: String = serde::Deserialize::deserialize(de)?;
        if s.len() != 64 {
            return Err(D::Error::custom(format!("expected 64 hex chars, got {}", s.len())));
        }
        let mut bytes = [0u8; 32];
        for (i, pair) in s.as_bytes().chunks(2).enumerate() {
            let Some((&hi_byte, &lo_byte)) = pair.first().zip(pair.get(1)) else {
                return Err(D::Error::custom("malformed hex chunk"));
            };
            let hi = hex_nibble(hi_byte).map_err(|e: HexError| D::Error::custom(e.to_string()))?;
            let lo = hex_nibble(lo_byte).map_err(|e: HexError| D::Error::custom(e.to_string()))?;
            if let Some(slot) = bytes.get_mut(i) {
                *slot = hi.wrapping_shl(4) | lo;
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn round_trip_hex() -> Result<(), crate::ReelError> {
        let raw: [u8; 32] = *blake3::hash(b"reel test").as_bytes();
        let h = Hash::from_bytes(raw);
        let recovered = Hash::from_hex(&h.to_hex())?;
        assert_eq!(h, recovered);
        Ok(())
    }

    #[test]
    fn from_blake3() {
        let b3 = blake3::hash(b"hello");
        let h: Hash = b3.into();
        assert_eq!(h.as_bytes(), b3.as_bytes());
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(Hash::from_hex("not-hex-at-all").is_err());
        assert!(Hash::from_hex("abc").is_err()); // too short
    }

    #[test]
    fn serde_json_round_trip() -> Result<(), Box<dyn Error>> {
        let h = Hash::from_bytes(*blake3::hash(b"serde").as_bytes());
        let json = serde_json::to_string(&h)?;
        let h2: Hash = serde_json::from_str(&json)?;
        assert_eq!(h, h2);
        Ok(())
    }
}
