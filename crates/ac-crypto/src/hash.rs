//! 256-bit hashing and domain separation.

use sha3::Digest;

use crate::error::Error;

/// A 32-byte hash output.
pub type Hash256 = [u8; 32];

/// Required first word of every domain-separation context.
const CONTEXT_PREFIX: &str = "agentcoin";

/// BLAKE3 with a 32-byte output (general-purpose hash).
#[must_use]
pub fn blake3_256(data: &[u8]) -> Hash256 {
    *blake3::hash(data).as_bytes()
}

/// SHA3-256 (for interoperability).
#[must_use]
pub fn sha3_256(data: &[u8]) -> Hash256 {
    sha3::Sha3_256::digest(data).into()
}

/// Domain-separated hash: BLAKE3 in `derive_key` mode keyed by `context`.
///
/// `context` must have the form `agentcoin <YYYY-MM> <purpose> v<version>`, e.g.
/// `agentcoin 2026-09 account-id v1`, and must be registered in the crate README.
///
/// # Errors
///
/// Returns [`Error::InvalidContext`] if `context` does not follow that format.
pub fn derive(context: &str, data: &[u8]) -> Result<Hash256, Error> {
    validate_context(context)?;
    Ok(blake3::derive_key(context, data))
}

/// Checks the `agentcoin <YYYY-MM> <purpose> v<version>` format.
///
/// # Errors
///
/// Returns [`Error::InvalidContext`] if the format is not followed.
pub fn validate_context(context: &str) -> Result<(), Error> {
    let mut parts = context.split(' ');
    let (Some(prefix), Some(date), Some(purpose), Some(version), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(Error::InvalidContext);
    };
    let ok = prefix == CONTEXT_PREFIX
        && is_year_month(date)
        && is_purpose(purpose)
        && is_version(version);
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidContext)
    }
}

fn is_year_month(s: &str) -> bool {
    let b = s.as_bytes();
    match b {
        [y0, y1, y2, y3, b'-', m0, m1] => {
            [y0, y1, y2, y3, m0, m1].iter().all(|c| c.is_ascii_digit())
                && matches!((m0, m1), (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2'))
        }
        _ => false,
    }
}

fn is_purpose(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn is_version(s: &str) -> bool {
    match s.strip_prefix('v') {
        Some(n) => !n.is_empty() && !n.starts_with('0') && n.bytes().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Requirement "域分离哈希" / Scenario "不同域输出不同".
    #[test]
    fn different_contexts_give_different_outputs() {
        let a = derive("agentcoin 2026-09 test-a v1", b"x").unwrap();
        let b = derive("agentcoin 2026-09 test-b v1", b"x").unwrap();
        assert_ne!(a, b);
    }

    // Scenario "上下文格式校验".
    #[test]
    fn rejects_malformed_contexts() {
        for bad in [
            "",
            "agentcoin",
            "other 2026-09 purpose v1",
            "agentcoin 2026-13 purpose v1",
            "agentcoin 2026-9 purpose v1",
            "agentcoin 2026-09 Purpose v1",
            "agentcoin 2026-09 purpose 1",
            "agentcoin 2026-09 purpose v0",
            "agentcoin 2026-09 purpose v1 extra",
            "agentcoin  2026-09 purpose v1",
        ] {
            assert_eq!(derive(bad, b"x"), Err(Error::InvalidContext), "{bad:?}");
        }
        assert!(validate_context("agentcoin 2026-09 account-id v1").is_ok());
        assert!(validate_context("agentcoin 2031-12 bft-vote v12").is_ok());
    }
}
