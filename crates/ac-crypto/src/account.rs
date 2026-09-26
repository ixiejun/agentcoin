//! 32-byte account identifiers.

use crate::tagged::PqPublicKey;

/// Domain-separation context for account IDs. Consensus-critical: never change it.
pub const ACCOUNT_ID_CONTEXT: &str = "agentcoin 2026-09 account-id v1";

/// A 32-byte account identifier.
///
/// Derived once from the account's first public key and unchanged by later key rotation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub [u8; 32]);

impl AccountId {
    /// The raw 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for AccountId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("AccountId(")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        f.write_str(")")
    }
}

/// Derives the account ID of a public key:
/// `BLAKE3-derive_key(ACCOUNT_ID_CONTEXT, alg_id ‖ raw_public_key)` (1-byte AlgId).
#[must_use]
pub fn account_id(public_key: &PqPublicKey) -> AccountId {
    derive_from_parts(public_key.alg().id(), public_key.as_bytes())
}

pub(crate) fn derive_from_parts(alg_id: u8, raw: &[u8]) -> AccountId {
    let mut hasher = blake3::Hasher::new_derive_key(ACCOUNT_ID_CONTEXT);
    hasher.update(&[alg_id]);
    hasher.update(raw);
    AccountId(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_well_formed() {
        assert!(crate::hash::validate_context(ACCOUNT_ID_CONTEXT).is_ok());
    }

    // Requirement "账户 ID 派生" / Scenario "算法参与派生".
    #[test]
    fn algorithm_id_is_part_of_the_derivation() {
        let raw = [9u8; 64];
        assert_ne!(derive_from_parts(0x01, &raw), derive_from_parts(0x02, &raw));
    }

    #[test]
    fn matches_the_documented_formula() {
        let raw = [3u8; 16];
        let mut material = alloc::vec::Vec::new();
        material.push(0x01);
        material.extend_from_slice(&raw);
        let expected = crate::hash::derive(ACCOUNT_ID_CONTEXT, &material).unwrap();
        assert_eq!(derive_from_parts(0x01, &raw).0, expected);
    }
}
