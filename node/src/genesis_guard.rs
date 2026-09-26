//! Constitution layer 1, first node-side check (decision D9, no premine): a **live** chain's
//! genesis must allocate no ATC. This runs in the node client, independently of the runtime,
//! and reads only published well-known storage keys (red line 3). It is a pure function so it
//! can move into `ac-invariants` with the full invariant checker in M3.

use parity_scale_codec::Decode;
use sp_core::storage::Storage;
use sp_io::hashing::twox_128;

/// Why a genesis is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenesisError {
    /// `Balances::TotalIssuance` is not zero.
    NonZeroIssuance(u128),
    /// An account holds a balance.
    EndowedAccount,
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
            Self::Malformed(what) => write!(f, "malformed genesis value: {what}"),
        }
    }
}

/// Storage key of `Balances::TotalIssuance` (well-known, never renamed).
#[must_use]
pub fn total_issuance_key() -> Vec<u8> {
    [twox_128(b"Balances"), twox_128(b"TotalIssuance")].concat()
}

/// Storage prefix of `System::Account` (well-known, never renamed).
#[must_use]
pub fn system_account_prefix() -> Vec<u8> {
    [twox_128(b"System"), twox_128(b"Account")].concat()
}

type AccountInfo = frame_system::AccountInfo<u32, pallet_balances::AccountData<u128>>;

/// Checks that a genesis state allocates no ATC: zero total issuance and no account with a free
/// or reserved balance.
///
/// # Errors
///
/// The first violation as a [`GenesisError`].
pub fn check_zero_issuance(storage: &Storage) -> Result<(), GenesisError> {
    if let Some(raw) = storage.top.get(&total_issuance_key()) {
        let issuance =
            u128::decode(&mut &raw[..]).map_err(|_| GenesisError::Malformed("TotalIssuance"))?;
        if issuance != 0 {
            return Err(GenesisError::NonZeroIssuance(issuance));
        }
    }
    let prefix = system_account_prefix();
    for (key, raw) in &storage.top {
        if key.starts_with(&prefix) {
            let info = AccountInfo::decode(&mut &raw[..])
                .map_err(|_| GenesisError::Malformed("System::Account"))?;
            if info.data.free != 0 || info.data.reserved != 0 {
                return Err(GenesisError::EndowedAccount);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parity_scale_codec::Encode;

    fn account_key(id: u8) -> Vec<u8> {
        let mut key = system_account_prefix();
        key.extend([id; 48]);
        key
    }

    fn account(free: u128) -> Vec<u8> {
        let mut info = AccountInfo::default();
        info.data.free = free;
        info.encode()
    }

    // Requirement "正式链创世零发行" / Scenario "零发行的正式链规格".
    #[test]
    fn zero_issuance_passes() {
        let mut storage = Storage::default();
        storage.top.insert(total_issuance_key(), 0u128.encode());
        storage.top.insert(account_key(1), account(0));
        assert_eq!(check_zero_issuance(&storage), Ok(()));
    }

    // Requirement "正式链创世零发行" / Scenario "带预挖的正式链规格".
    #[test]
    fn premine_is_rejected() {
        let one_atc = ac_runtime::ATC;
        let mut storage = Storage::default();
        storage.top.insert(total_issuance_key(), one_atc.encode());
        storage.top.insert(account_key(1), account(one_atc));
        assert_eq!(
            check_zero_issuance(&storage),
            Err(GenesisError::NonZeroIssuance(one_atc))
        );
        assert!(
            check_zero_issuance(&storage)
                .unwrap_err()
                .to_string()
                .contains("genesis issuance")
        );
    }

    // A balance without matching issuance (hand-edited raw spec) is also rejected.
    #[test]
    fn balance_without_issuance_is_rejected() {
        let mut storage = Storage::default();
        storage.top.insert(account_key(2), account(5));
        assert_eq!(
            check_zero_issuance(&storage),
            Err(GenesisError::EndowedAccount)
        );
    }

    // The well-known keys are what the runtime actually uses.
    #[test]
    fn well_known_keys_match_the_runtime() {
        assert_eq!(
            total_issuance_key(),
            pallet_balances::TotalIssuance::<ac_runtime::Runtime>::hashed_key().to_vec()
        );
        assert_eq!(system_account_prefix(), {
            use frame_support::storage::StoragePrefixedMap;
            frame_system::Account::<ac_runtime::Runtime>::final_prefix().to_vec()
        });
    }
}
