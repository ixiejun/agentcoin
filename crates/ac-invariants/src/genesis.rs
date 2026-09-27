//! Start-up checks of a chain's genesis (decision D9; spec node/invariants "启动时校验创世参数"
//! and node/chain-spec "正式链创世零发行"). Moved here from the M1 node's `genesis_guard`.

use ac_primitives::emission::{CAP, EmissionError, EmissionSchedule};
use parity_scale_codec::{Decode, Input};

use crate::keys;

/// Parameters fixed at genesis that the per-block checks need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenesisParams {
    /// Emission curve for the genesis emission epoch length.
    pub schedule: EmissionSchedule,
    /// Total issuance at genesis (non-zero only on development chains).
    pub genesis_issuance: u128,
}

/// Why a genesis is rejected.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenesisError {
    /// A live chain's genesis issuance is not zero (no premine, D9).
    NonZeroIssuance(u128),
    /// A live chain's genesis gives an account a balance.
    EndowedAccount,
    /// The genesis issuance exceeds the supply cap.
    IssuanceAboveCap(u128),
    /// The emission epoch length is missing.
    MissingEpochLength,
    /// The emission epoch length is zero or does not divide the four-year period.
    InvalidEpochLength(u64),
    /// A live chain has no PoA admin members.
    NoAdminMembers,
    /// A well-known value does not decode.
    Malformed(&'static str),
}

impl core::fmt::Display for GenesisError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NonZeroIssuance(n) => write!(
                f,
                "live chain genesis issuance must be zero (no premine, D9), found {n}"
            ),
            Self::EndowedAccount => f.write_str(
                "live chain genesis issuance must be zero (no premine, D9): an account holds a balance",
            ),
            Self::IssuanceAboveCap(n) => {
                write!(f, "genesis issuance {n} exceeds the 21,000,000 ATC cap")
            }
            Self::MissingEpochLength => {
                f.write_str("the chain spec sets no emission epoch length (Emission::EpochLength)")
            }
            Self::InvalidEpochLength(l) => {
                write!(f, "{}", EmissionError::InvalidEpochLength(*l))
            }
            Self::NoAdminMembers => f.write_str("a live chain needs at least one PoA admin member"),
            Self::Malformed(what) => write!(f, "malformed genesis value: {what}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for GenesisError {}

/// The balance part of `System::Account` values: `AccountInfo<u32, AccountData<u128>>` is
/// four `u32` counters followed by free, reserved and frozen balances and the flags.
struct AccountBalances {
    free: u128,
    reserved: u128,
}

impl Decode for AccountBalances {
    fn decode<I: Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
        let _counters = <[u32; 4]>::decode(input)?;
        let free = u128::decode(input)?;
        let reserved = u128::decode(input)?;
        let _frozen_and_flags = <[u128; 2]>::decode(input)?;
        Ok(Self { free, reserved })
    }
}

fn decode<T: Decode>(raw: &[u8], what: &'static str) -> Result<T, GenesisError> {
    let mut input = raw;
    let value = T::decode(&mut input).map_err(|_| GenesisError::Malformed(what))?;
    if input.is_empty() {
        Ok(value)
    } else {
        Err(GenesisError::Malformed(what))
    }
}

/// Checks a genesis state given as its top-level `(key, value)` pairs and returns the
/// parameters for block checks. Every chain needs a valid emission epoch length and an
/// issuance within the cap; a `live` chain must also allocate no ATC and name PoA admin
/// members.
///
/// # Errors
///
/// The first violation as a [`GenesisError`].
pub fn check_genesis<'a>(
    entries: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    live: bool,
) -> Result<GenesisParams, GenesisError> {
    let mut issuance = 0u128;
    let mut epoch_length = None;
    let mut members = 0usize;
    let mut endowed = false;
    for (key, value) in entries {
        if key == keys::TOTAL_ISSUANCE.as_slice() {
            issuance = decode(value, "Balances::TotalIssuance")?;
        } else if key == keys::EMISSION_EPOCH_LENGTH.as_slice() {
            epoch_length = Some(decode::<u64>(value, "Emission::EpochLength")?);
        } else if key == keys::POA_COUNCIL_MEMBERS.as_slice() {
            members = decode::<alloc::vec::Vec<[u8; 32]>>(value, "PoaCouncil::Members")?.len();
        } else if key.starts_with(&keys::SYSTEM_ACCOUNT_PREFIX) {
            let account: AccountBalances = decode(value, "System::Account")?;
            endowed |= account.free != 0 || account.reserved != 0;
        }
    }
    if live {
        if issuance != 0 {
            return Err(GenesisError::NonZeroIssuance(issuance));
        }
        if endowed {
            return Err(GenesisError::EndowedAccount);
        }
    }
    if issuance > CAP {
        return Err(GenesisError::IssuanceAboveCap(issuance));
    }
    let length = epoch_length.ok_or(GenesisError::MissingEpochLength)?;
    let schedule =
        EmissionSchedule::new(length).map_err(|_| GenesisError::InvalidEpochLength(length))?;
    if live && members == 0 {
        return Err(GenesisError::NoAdminMembers);
    }
    Ok(GenesisParams {
        schedule,
        genesis_issuance: issuance,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

    use super::*;
    use alloc::vec::Vec;
    use parity_scale_codec::Encode;

    const ATC: u128 = ac_primitives::emission::UNITS;

    fn account_key(id: u8) -> Vec<u8> {
        let mut key = keys::SYSTEM_ACCOUNT_PREFIX.to_vec();
        key.extend([id; 48]);
        key
    }

    fn account(free: u128) -> Vec<u8> {
        ([0u32; 4], free, 0u128, [0u128; 2]).encode()
    }

    fn state(issuance: u128, length: Option<u64>, members: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut s = vec![(keys::TOTAL_ISSUANCE.to_vec(), issuance.encode())];
        if let Some(l) = length {
            s.push((keys::EMISSION_EPOCH_LENGTH.to_vec(), l.encode()));
        }
        s.push((
            keys::POA_COUNCIL_MEMBERS.to_vec(),
            vec![[1u8; 32]; members].encode(),
        ));
        s
    }

    fn check(s: &[(Vec<u8>, Vec<u8>)], live: bool) -> Result<GenesisParams, GenesisError> {
        check_genesis(s.iter().map(|(k, v)| (k.as_slice(), v.as_slice())), live)
    }

    // Scenario "零发行的正式链规格".
    #[test]
    fn zero_issuance_passes() {
        let mut s = state(0, Some(3_600), 3);
        s.push((account_key(1), account(0)));
        let params = check(&s, true).unwrap();
        assert_eq!(params.schedule.epoch_length(), 3_600);
        assert_eq!(params.genesis_issuance, 0);
    }

    // Scenario "带预挖的正式链规格".
    #[test]
    fn premine_is_rejected() {
        let mut s = state(ATC, Some(3_600), 3);
        s.push((account_key(1), account(ATC)));
        let err = check(&s, true).unwrap_err();
        assert_eq!(err, GenesisError::NonZeroIssuance(ATC));
        assert!(err.to_string().contains("genesis issuance"));
        // Development chains may allocate balances.
        assert_eq!(check(&s, false).unwrap().genesis_issuance, ATC);
    }

    // A balance without matching issuance (hand-edited raw spec) is also rejected.
    #[test]
    fn balance_without_issuance_is_rejected() {
        let mut s = state(0, Some(3_600), 1);
        s.push((account_key(2), account(5)));
        assert_eq!(check(&s, true).unwrap_err(), GenesisError::EndowedAccount);
    }

    // Scenarios "链规格缺少纪元长度" and "非法纪元长度".
    #[test]
    fn epoch_length_is_required_and_valid() {
        assert_eq!(
            check(&state(0, None, 1), false).unwrap_err(),
            GenesisError::MissingEpochLength
        );
        assert_eq!(
            check(&state(0, Some(7), 1), false).unwrap_err(),
            GenesisError::InvalidEpochLength(7)
        );
        assert!(
            check(&state(0, None, 1), false)
                .unwrap_err()
                .to_string()
                .contains("emission epoch length")
        );
    }

    // Scenario "缺少成员的正式链创世" (node side).
    #[test]
    fn live_chain_needs_admin_members() {
        assert_eq!(
            check(&state(0, Some(3_600), 0), true).unwrap_err(),
            GenesisError::NoAdminMembers
        );
        assert!(check(&state(0, Some(3_600), 0), false).is_ok());
    }

    #[test]
    fn issuance_above_cap_and_malformed_values() {
        assert_eq!(
            check(&state(CAP + 1, Some(10), 1), false).unwrap_err(),
            GenesisError::IssuanceAboveCap(CAP + 1)
        );
        let bad = vec![(keys::EMISSION_EPOCH_LENGTH.to_vec(), vec![1, 2, 3])];
        assert_eq!(
            check(&bad, false).unwrap_err(),
            GenesisError::Malformed("Emission::EpochLength")
        );
    }
}
