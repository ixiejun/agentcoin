#![doc = include_str!("../README.md")]

pub mod amount;
pub mod audit;
pub mod client;
pub mod evm;
pub mod http;
pub mod kem_key;
pub mod market;
pub mod ops;
pub mod proxy;
pub mod sealed_http;
pub mod wallet;
pub mod work;

pub use client::{Inclusion, NodeClient};
pub use wallet::{Created, Wallet, parse_alg};

/// Parses an `atc1…` address; any checksum, prefix or length error is rejected.
///
/// # Errors
///
/// A message naming the problem and the address.
pub fn parse_address(address: &str) -> anyhow::Result<sp_runtime::AccountId32> {
    let bytes = ac_primitives::decode_address(address.trim())
        .map_err(|e| anyhow::anyhow!("{e}: {address}"))?;
    Ok(sp_runtime::AccountId32::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::parse_address;

    // Requirement "地址显示" / Scenarios "地址往返" and "地址输错".
    #[test]
    fn addresses_round_trip_and_typos_are_rejected() {
        let address = ac_primitives::encode_address(&[5u8; 32]);
        assert_eq!(
            parse_address(&address).unwrap(),
            sp_runtime::AccountId32::new([5u8; 32])
        );
        let mut typo: Vec<char> = address.chars().collect();
        let i = typo.len() - 3;
        typo[i] = if typo[i] == 'q' { 'p' } else { 'q' };
        let typo: String = typo.into_iter().collect();
        assert!(parse_address(&typo).is_err());
    }
}
