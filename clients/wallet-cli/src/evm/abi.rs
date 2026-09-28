//! Solidity ABI encoding of call data and constructor arguments from text, for `evm send` and
//! `evm deploy` (design D10 of m4-evm).
//!
//! Types and values are parsed and encoded by `alloy-dyn-abi`; this module only splits function
//! signatures and derives selectors. Every Solidity type `alloy-dyn-abi` knows is accepted
//! (static types, `bytes`, `string`, arrays and tuples), a superset of what the design requires.

use alloy_core::dyn_abi::{DynSolType, DynSolValue};
use alloy_core::primitives::keccak256;
use anyhow::{Context, Result, bail};

/// A parsed function (or constructor) signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Function name; empty for `constructor(...)` and bare `(...)` lists.
    pub name: String,
    /// Parameter types.
    pub params: Vec<DynSolType>,
}

impl Signature {
    /// Parses `name(type,...)`, `constructor(type,...)` or `(type,...)`.
    ///
    /// # Errors
    ///
    /// Malformed signatures or unknown types.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let open = text
            .find('(')
            .with_context(|| format!("signature {text:?} has no parameter list"))?;
        let (name, list) = text.split_at(open);
        let name = name.trim();
        if !name.is_empty() && !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("invalid function name {name:?}");
        }
        let params =
            match DynSolType::parse(list).with_context(|| format!("invalid types {list}"))? {
                DynSolType::Tuple(params) => params,
                // A one-element list parses as that element.
                single => vec![single],
            };
        let name = if name == "constructor" { "" } else { name };
        Ok(Self {
            name: name.to_owned(),
            params,
        })
    }

    /// The canonical form, e.g. `transfer(address,uint256)`.
    #[must_use]
    pub fn canonical(&self) -> String {
        let types: Vec<String> = self
            .params
            .iter()
            .map(DynSolType::sol_type_name)
            .map(Into::into)
            .collect();
        format!("{}({})", self.name, types.join(","))
    }

    /// The 4-byte selector: the first bytes of `keccak256(canonical)`.
    #[must_use]
    pub fn selector(&self) -> [u8; 4] {
        let hash = keccak256(self.canonical().as_bytes());
        let mut selector = [0u8; 4];
        selector.copy_from_slice(hash.get(..4).unwrap_or(&[0; 4]));
        selector
    }

    /// ABI-encodes `args` (text, one per parameter) as the parameters of this signature.
    ///
    /// # Errors
    ///
    /// A wrong number of arguments or an argument that does not parse as its type.
    pub fn encode_params(&self, args: &[String]) -> Result<Vec<u8>> {
        if args.len() != self.params.len() {
            bail!(
                "{} expects {} argument(s), got {}",
                self.canonical(),
                self.params.len(),
                args.len()
            );
        }
        let values = self
            .params
            .iter()
            .zip(args)
            .enumerate()
            .map(|(i, (ty, arg))| {
                ty.coerce_str(arg).with_context(|| {
                    format!(
                        "argument {} ({arg:?}) is not a valid {}",
                        i + 1,
                        ty.sol_type_name()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(DynSolValue::Tuple(values).abi_encode_params())
    }

    /// Call data: the selector followed by the encoded arguments.
    ///
    /// # Errors
    ///
    /// See [`Signature::encode_params`]; also a signature without a function name.
    pub fn calldata(&self, args: &[String]) -> Result<Vec<u8>> {
        if self.name.is_empty() {
            bail!("a call needs a function name, e.g. transfer(address,uint256)");
        }
        let mut data = self.selector().to_vec();
        data.extend(self.encode_params(args)?);
        Ok(data)
    }
}

/// Parses hexadecimal bytes with or without a `0x` prefix.
///
/// # Errors
///
/// Invalid hexadecimal.
pub fn parse_hex(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    let digits = text.strip_prefix("0x").unwrap_or(text);
    hex::decode(digits).with_context(|| format!("invalid hex {text:?}"))
}

/// A readable reason for revert data: `Error(string)` messages, `Panic(uint256)` codes, or the
/// raw bytes of custom errors.
#[must_use]
pub fn revert_reason(data: &[u8]) -> String {
    const ERROR: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];
    const PANIC: [u8; 4] = [0x4e, 0x48, 0x7b, 0x71];
    let (Some(selector), Some(body)) = (data.get(..4), data.get(4..)) else {
        return if data.is_empty() {
            "reverted without data".to_owned()
        } else {
            format!("reverted with 0x{}", hex::encode(data))
        };
    };
    if selector == ERROR
        && let Ok(DynSolValue::String(message)) = DynSolType::String.abi_decode_params(body)
    {
        return format!("reverted: {message}");
    }
    if selector == PANIC
        && let Ok(DynSolValue::Uint(code, _)) = DynSolType::Uint(256).abi_decode_params(body)
    {
        return format!("panicked with code {code:#x}");
    }
    format!("reverted with 0x{}", hex::encode(data))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    // Task 6.2: byte-for-byte against Foundry. Expected values were produced with
    // `cast calldata` / `cast abi-encode` from Foundry 1.7.1 (commit 4072e48) and are fixed here.
    #[test]
    fn matches_cast_calldata() {
        let cases: [(&str, &[&str], &str); 5] = [
            (
                "transfer(address,uint256)",
                &["0x00000000000000000000000000000000000000aa", "1000"],
                include_str!("vectors/transfer.hex"),
            ),
            (
                "approve(address,uint256)",
                &[
                    "0x000000000000000000000000000000000000beef",
                    "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                ],
                include_str!("vectors/approve.hex"),
            ),
            (
                "verify(uint8,bytes,bytes,bytes)",
                &["2", "0x0102", "0x68656c6c6f", "0x"],
                include_str!("vectors/verify.hex"),
            ),
            (
                "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)",
                &[
                    "1000",
                    "0",
                    "[0x00000000000000000000000000000000000000a1,0x00000000000000000000000000000000000000b2]",
                    "0x00000000000000000000000000000000000000c3",
                    "99999999999",
                ],
                include_str!("vectors/swap.hex"),
            ),
            (
                "set(bool,int256,bytes32,string)",
                &[
                    "true",
                    "-5",
                    "0x1111111111111111111111111111111111111111111111111111111111111111",
                    "AgentCoin",
                ],
                include_str!("vectors/set.hex"),
            ),
        ];
        for (sig, values, expected) in cases {
            let data = Signature::parse(sig)
                .unwrap()
                .calldata(&args(values))
                .unwrap();
            assert_eq!(format!("0x{}", hex::encode(data)), expected.trim(), "{sig}");
        }
    }

    #[test]
    fn matches_cast_abi_encode_for_constructors() {
        let expected = include_str!("vectors/constructor.hex");
        for sig in [
            "constructor(string,string,uint256)",
            "(string,string,uint256)",
        ] {
            let encoded = Signature::parse(sig)
                .unwrap()
                .encode_params(&args(&["Token", "TKN", "1000000000000000000000"]))
                .unwrap();
            assert_eq!(format!("0x{}", hex::encode(encoded)), expected.trim());
        }
    }

    #[test]
    fn signatures_are_canonical() {
        let sig = Signature::parse("transfer(address, uint)").unwrap();
        assert_eq!(sig.canonical(), "transfer(address,uint256)");
        assert_eq!(sig.selector(), [0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(Signature::parse("f(uint8)").unwrap().params.len(), 1);
        assert!(Signature::parse("transfer").is_err());
        assert!(Signature::parse("bad name(uint8)").is_err());
        assert!(Signature::parse("f(uint7)").is_err());
    }

    #[test]
    fn wrong_arguments_are_refused() {
        let sig = Signature::parse("transfer(address,uint256)").unwrap();
        assert!(
            sig.calldata(&args(&["0x00000000000000000000000000000000000000aa"]))
                .is_err()
        );
        assert!(sig.calldata(&args(&["not an address", "1"])).is_err());
        assert!(
            sig.calldata(&args(&["0x00000000000000000000000000000000000000aa", "-1"]))
                .is_err()
        );
        assert!(
            Signature::parse("(uint256)")
                .unwrap()
                .calldata(&args(&["1"]))
                .is_err()
        );
    }

    #[test]
    fn revert_reasons_are_readable() {
        let no = parse_hex(concat!(
            "08c379a0",
            "0000000000000000000000000000000000000000000000000000000000000020",
            "0000000000000000000000000000000000000000000000000000000000000002",
            "6e6f000000000000000000000000000000000000000000000000000000000000"
        ))
        .unwrap();
        assert_eq!(revert_reason(&no), "reverted: no");
        let panic = parse_hex(concat!(
            "4e487b71",
            "0000000000000000000000000000000000000000000000000000000000000011"
        ))
        .unwrap();
        assert_eq!(revert_reason(&panic), "panicked with code 0x11");
        assert_eq!(revert_reason(&[]), "reverted without data");
        assert_eq!(revert_reason(&[0xde, 0xad]), "reverted with 0xdead");
        assert_eq!(
            revert_reason(&[0x12, 0x34, 0x56, 0x78]),
            "reverted with 0x12345678"
        );
    }
}
