//! Published well-known storage keys (red line 3). They are part of the constitution: the
//! runtime must keep them readable under these exact keys and encodings forever, and changing
//! them means a new node client (a hard fork). Each is `twox128(pallet) ‖ twox128(item)`.

/// `Balances::TotalIssuance`: total issuance, SCALE `u128`.
pub const TOTAL_ISSUANCE: [u8; 32] =
    hex32("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80");

/// `Emission::TotalBurned`: cumulative burned amount, SCALE `u128`, never decreasing.
pub const TOTAL_BURNED: [u8; 32] =
    hex32("93916af563e2313a574d6cd48caec1a5dc03e46ac534de48716fb9881ad9991c");

/// `Emission::EpochLength`: emission epoch length in blocks, SCALE `u64` (genesis parameter).
pub const EMISSION_EPOCH_LENGTH: [u8; 32] =
    hex32("93916af563e2313a574d6cd48caec1a59c3fd13ec9ab7a5533b6c5ccca93a44f");

/// `System::Account` map prefix; values are `AccountInfo` with `AccountData<u128>`.
pub const SYSTEM_ACCOUNT_PREFIX: [u8; 32] =
    hex32("26aa394eea5630e07c48ae0c9558cef7b99d880ec681799c0cf30e8886371da9");

/// `PoaCouncil::Members`: PoA admin members, SCALE `Vec<AccountId32>`.
pub const POA_COUNCIL_MEMBERS: [u8; 32] =
    hex32("0a7e2b603d0e3b9627cde4d35083b551ba7fb8745735dc3be2a2c61a72c39e78");

/// `ValidatorSet::Phase`: validator phase, SCALE `u8` enum (`0` PoA, `1` PoS).
pub const PHASE: [u8; 32] =
    hex32("7d9fe37370ac390779f35763d98106e89551e4458a23409750643d92b070d46e");

/// `ValidatorSet::QualifiedSince`: start of the current qualified run, SCALE `Option<u64>`.
pub const QUALIFIED_SINCE: [u8; 32] =
    hex32("7d9fe37370ac390779f35763d98106e84787dc1e44fd8920d87604ad7d48e66d");

/// `ValidatorSet::PoaAuthorities`: PoA roster, SCALE vector of tagged public keys.
pub const POA_AUTHORITIES: [u8; 32] =
    hex32("7d9fe37370ac390779f35763d98106e83555fe80cefa47467018b55b4364cb05");

/// `ValidatorSet::TransitionParams`: switch parameters, SCALE
/// `(u32 stake_bps, u32 min_candidates, u64 min_height, u64 sustain_blocks)` (genesis).
pub const TRANSITION_PARAMS: [u8; 32] =
    hex32("7d9fe37370ac390779f35763d98106e8e3bf0848d2bd9cfafe2306ac94120860");

/// `ValidatorSet::EpochLength`: validator epoch length in blocks, SCALE `u64` (genesis).
pub const VALIDATOR_EPOCH_LENGTH: [u8; 32] =
    hex32("7d9fe37370ac390779f35763d98106e89c3fd13ec9ab7a5533b6c5ccca93a44f");

/// `StakingPos::Ledger` map prefix; the account ID follows unhashed (`Identity`), values are
/// staking ledgers whose first field is the active amount (SCALE `u128`).
pub const STAKING_LEDGER_PREFIX: [u8; 32] =
    hex32("63b1880c16c0ab9a119f647a27e420c4422adb579f1dbf4f3886c5cfa3bb8cc4");

/// `StakingPos::Candidates` map prefix; the account ID follows unhashed (`Identity`), values
/// are SCALE `ac_primitives::staking::CandidateRecord`.
pub const STAKING_CANDIDATES_PREFIX: [u8; 32] =
    hex32("63b1880c16c0ab9a119f647a27e420c4948ece45793d7f15c9c0b9574ddbc665");

/// Parses 64 hex digits at compile time. An invalid literal fails compilation.
// Const evaluation over a fixed 64-digit literal: indices stay in range and nibbles fit a byte.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
const fn hex32(s: &str) -> [u8; 32] {
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
        i += 1;
    }
    out
}

// Only evaluated in `const` items: the arms keep every result in 0..16, and the fallback
// `panic!` is a compile-time error for a malformed literal, never a run-time panic.
#[allow(clippy::arithmetic_side_effects, clippy::panic)]
const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("invalid hex digit in a well-known key"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use sp_io::hashing::twox_128;

    fn key(pallet: &str, item: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(&twox_128(pallet.as_bytes()));
        out[16..].copy_from_slice(&twox_128(item.as_bytes()));
        out
    }

    #[test]
    fn keys_are_twox128_of_their_names() {
        assert_eq!(TOTAL_ISSUANCE, key("Balances", "TotalIssuance"));
        assert_eq!(TOTAL_BURNED, key("Emission", "TotalBurned"));
        assert_eq!(EMISSION_EPOCH_LENGTH, key("Emission", "EpochLength"));
        assert_eq!(SYSTEM_ACCOUNT_PREFIX, key("System", "Account"));
        assert_eq!(POA_COUNCIL_MEMBERS, key("PoaCouncil", "Members"));
        assert_eq!(PHASE, key("ValidatorSet", "Phase"));
        assert_eq!(QUALIFIED_SINCE, key("ValidatorSet", "QualifiedSince"));
        assert_eq!(POA_AUTHORITIES, key("ValidatorSet", "PoaAuthorities"));
        assert_eq!(TRANSITION_PARAMS, key("ValidatorSet", "TransitionParams"));
        assert_eq!(VALIDATOR_EPOCH_LENGTH, key("ValidatorSet", "EpochLength"));
        assert_eq!(STAKING_LEDGER_PREFIX, key("StakingPos", "Ledger"));
        assert_eq!(STAKING_CANDIDATES_PREFIX, key("StakingPos", "Candidates"));
    }
}
