//! Human-readable account addresses: bech32m (BIP-350) with HRP `atc` over the 32-byte account ID.

use alloc::string::String;
use core::fmt;

use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32m, Hrp};
use sp_runtime::AccountId32;

/// Human-readable part of every AgentCoin address.
pub const ADDRESS_HRP: &str = "atc";

/// Why an address string was rejected.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// Not a valid bech32m string (bad characters, mixed case or checksum mismatch).
    Checksum,
    /// A valid bech32m string with a human-readable part other than `atc`.
    WrongPrefix,
    /// The payload is not exactly 32 bytes.
    WrongLength,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Checksum => "invalid address: checksum or character error",
            Self::WrongPrefix => "invalid address: prefix is not `atc`",
            Self::WrongLength => "invalid address: payload is not 32 bytes",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AddressError {}

/// Encodes a 32-byte account ID as an `atc1…` address.
#[must_use]
pub fn encode_address(account: &[u8; 32]) -> String {
    // The HRP is a valid constant and 32 bytes are far below the bech32m length limit, so
    // encoding cannot fail; an empty string would only signal a broken dependency.
    bech32::encode::<Bech32m>(Hrp::parse_unchecked(ADDRESS_HRP), account).unwrap_or_default()
}

/// Parses an `atc1…` address back to the 32-byte account ID.
///
/// # Errors
///
/// [`AddressError`] describing why the string is not a valid AgentCoin address.
pub fn decode_address(address: &str) -> Result<[u8; 32], AddressError> {
    let checked = CheckedHrpstring::new::<Bech32m>(address).map_err(|_| AddressError::Checksum)?;
    if checked.hrp() != Hrp::parse_unchecked(ADDRESS_HRP) {
        return Err(AddressError::WrongPrefix);
    }
    let mut out = [0u8; 32];
    let mut len = 0usize;
    for byte in checked.byte_iter() {
        let slot = out.get_mut(len).ok_or(AddressError::WrongLength)?;
        *slot = byte;
        len = len.saturating_add(1);
    }
    if len != out.len() {
        return Err(AddressError::WrongLength);
    }
    Ok(out)
}

/// Converts an `ac-crypto` account ID to the runtime's account type.
#[must_use]
pub fn to_runtime_account(account: &ac_crypto::AccountId) -> AccountId32 {
    AccountId32::new(*account.as_bytes())
}

/// Converts the runtime's account type to an `ac-crypto` account ID.
#[must_use]
pub fn from_runtime_account(account: &AccountId32) -> ac_crypto::AccountId {
    ac_crypto::AccountId(*<AccountId32 as AsRef<[u8; 32]>>::as_ref(account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::{any, proptest};

    /// BIP-350 Bech32m test vectors, bitcoin/bips commit 02bebeb5ef53dc21a52d5706c6b4c17b6af6c1cc,
    /// `bip-0350.mediawiki`, section "Test vectors for Bech32m".
    const BIP350_VALID: [&str; 7] = [
        "A1LQFN3A",
        "a1lqfn3a",
        "an83characterlonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11sg7hg6",
        "abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx",
        "11llllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllludsr8",
        "split1checkupstagehandshakeupstreamerranterredcaperredlc445v",
        "?1v759aa",
    ];
    const BIP350_INVALID: [&str; 11] = [
        "an84characterslonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11d6pts4",
        "qyrz8wqd2c9m",
        "1qyrz8wqd2c9m",
        "y1b0jsk6g",
        "lt1igcx5c0",
        "in1muywd",
        "mm1crxm3i",
        "au1s5cgom",
        "M1VUXWEZ",
        "16plkw9",
        "1p2gdwpf",
    ];

    #[test]
    fn bip350_checksum_vectors() {
        for s in BIP350_VALID {
            assert!(
                CheckedHrpstring::new::<Bech32m>(s).is_ok(),
                "{s} should be valid"
            );
        }
        for s in BIP350_INVALID {
            assert!(
                CheckedHrpstring::new::<Bech32m>(s).is_err(),
                "{s} should be invalid"
            );
        }
    }

    /// Fixed address of the development account `alice` (ML-DSA-44), from
    /// `crates/ac-crypto/tests/vectors/wallet_keys.json`. Regression only: never change.
    #[test]
    fn fixed_address_vector() {
        let id: [u8; 32] =
            hex::decode("a8770b639d8aa43ec4372f2d41a0bc95e1d7f44126abb337539875db3dc2f5e9")
                .unwrap()
                .try_into()
                .unwrap();
        let address = encode_address(&id);
        assert!(address.starts_with("atc1"));
        assert_eq!(address, ALICE_ADDRESS);
        assert_eq!(decode_address(&address), Ok(id));
    }

    const ALICE_ADDRESS: &str = "atc14pmskcua32jra3ph9uk5rg9ujhsa0azpy64mxd6nnp6ak0wz7h5smqqxf0";

    // Scenario "地址输错": a single changed character is rejected.
    #[test]
    fn single_character_error_is_detected() {
        let address = encode_address(&[0x42; 32]);
        for i in 4..address.len() {
            let mut chars: alloc::vec::Vec<char> = address.chars().collect();
            chars[i] = if chars[i] == 'q' { 'p' } else { 'q' };
            let mutated: String = chars.into_iter().collect();
            assert_eq!(
                decode_address(&mutated),
                Err(AddressError::Checksum),
                "{mutated}"
            );
        }
    }

    #[test]
    fn wrong_prefix_and_length_are_rejected() {
        let other = bech32::encode::<Bech32m>(Hrp::parse_unchecked("btc"), &[1; 32]).unwrap();
        assert_eq!(decode_address(&other), Err(AddressError::WrongPrefix));
        let short = bech32::encode::<Bech32m>(Hrp::parse_unchecked("atc"), &[1; 31]).unwrap();
        assert_eq!(decode_address(&short), Err(AddressError::WrongLength));
        let long = bech32::encode::<Bech32m>(Hrp::parse_unchecked("atc"), &[1; 33]).unwrap();
        assert_eq!(decode_address(&long), Err(AddressError::WrongLength));
        // Legacy bech32 (BIP-173) checksums are not accepted.
        let legacy =
            bech32::encode::<bech32::Bech32>(Hrp::parse_unchecked("atc"), &[1; 32]).unwrap();
        assert_eq!(decode_address(&legacy), Err(AddressError::Checksum));
    }

    #[test]
    fn runtime_account_conversion_round_trips() {
        let id = ac_crypto::AccountId([9; 32]);
        assert_eq!(from_runtime_account(&to_runtime_account(&id)), id);
    }

    proptest! {
        // Scenario "地址往返".
        #[test]
        fn address_round_trip(bytes in any::<[u8; 32]>()) {
            proptest::prop_assert_eq!(decode_address(&encode_address(&bytes)), Ok(bytes));
        }
    }
}
