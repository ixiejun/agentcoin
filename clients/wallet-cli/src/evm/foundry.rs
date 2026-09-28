//! Foundry files: compiled artifacts (`forge build`) and script broadcasts (`forge script`,
//! `run-latest.json`), design D10 of m4-evm.

use anyhow::{Context, Result, bail};
use pallet_revive::{H160, U256};
use serde::Deserialize;

use super::abi::parse_hex;
use super::parse_address;

/// The EVM init code of a `forge build` artifact (`bytecode.object`).
///
/// # Errors
///
/// Malformed JSON, a missing or empty bytecode (interfaces and abstract contracts), or unlinked
/// library placeholders.
pub fn init_code(artifact: &str) -> Result<Vec<u8>> {
    let json: serde_json::Value = serde_json::from_str(artifact).context("malformed artifact")?;
    let object = match &json["bytecode"] {
        serde_json::Value::Object(bytecode) => bytecode.get("object").and_then(|o| o.as_str()),
        serde_json::Value::String(object) => Some(object.as_str()),
        _ => None,
    }
    .context("the artifact has no bytecode.object")?;
    if object.contains("__$") {
        bail!("the bytecode has unlinked library references; link them before deploying");
    }
    let code = parse_hex(object)?;
    if code.is_empty() {
        bail!("the artifact has no init code (an interface or abstract contract?)");
    }
    Ok(code)
}

/// What one broadcast transaction does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// Deploy `data` as init code; `expected` is the address the script predicted.
    Create {
        /// Predicted contract address.
        expected: Option<H160>,
    },
    /// Call `to` with `data`. `CREATE2` entries (a factory call, design D10) carry the address
    /// the factory is expected to create.
    Call {
        /// Callee.
        to: H160,
        /// Address a `CREATE2` entry predicts.
        expected: Option<H160>,
    },
}

/// One transaction of a broadcast, in script order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// Deployment or call.
    pub kind: StepKind,
    /// Value in the smallest unit.
    pub value: u128,
    /// Init code or call data.
    pub data: Vec<u8>,
}

#[derive(Deserialize)]
struct Broadcast {
    transactions: Vec<BroadcastTx>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BroadcastTx {
    transaction_type: String,
    contract_address: Option<String>,
    transaction: TxFields,
}

#[derive(Deserialize)]
struct TxFields {
    from: Option<String>,
    to: Option<String>,
    input: Option<String>,
    // Older Foundry versions name the input `data`.
    data: Option<String>,
    value: Option<String>,
}

fn value_of(text: Option<&str>) -> Result<u128> {
    let Some(text) = text else { return Ok(0) };
    let value = match text.strip_prefix("0x") {
        Some(digits) => U256::from_str_radix(digits, 16).ok(),
        None => U256::from_dec_str(text).ok(),
    }
    .with_context(|| format!("invalid value {text:?}"))?;
    u128::try_from(value).map_err(|_| anyhow::anyhow!("value {text} out of range"))
}

/// Reads a `run-latest.json` into steps. Every transaction must be sent from `sender` (run
/// `forge script --sender <ac-wallet evm address>`), otherwise the predicted addresses are
/// wrong and nothing is planned.
///
/// # Errors
///
/// Malformed files, unknown transaction types, or transactions from another sender.
pub fn broadcast_steps(json: &str, sender: H160) -> Result<Vec<Step>> {
    let broadcast: Broadcast = serde_json::from_str(json).context("malformed broadcast file")?;
    broadcast
        .transactions
        .into_iter()
        .enumerate()
        .map(|(i, tx)| {
            let n = i.saturating_add(1);
            if let Some(from) = &tx.transaction.from {
                let from = parse_address(from)?;
                if from != sender {
                    bail!(
                        "transaction {n} is from {from:?}, not the wallet's {sender:?}; \
                         rerun forge script with --sender set to the wallet's EVM address"
                    );
                }
            }
            let data = tx
                .transaction
                .input
                .as_deref()
                .or(tx.transaction.data.as_deref())
                .map(parse_hex)
                .transpose()?
                .unwrap_or_default();
            let value = value_of(tx.transaction.value.as_deref())?;
            let expected = tx
                .contract_address
                .as_deref()
                .map(parse_address)
                .transpose()?;
            let to = || -> Result<H160> {
                parse_address(
                    tx.transaction
                        .to
                        .as_deref()
                        .context("a call without `to`")?,
                )
            };
            let kind = match tx.transaction_type.as_str() {
                "CREATE" => StepKind::Create { expected },
                "CALL" => StepKind::Call {
                    to: to()?,
                    expected: None,
                },
                "CREATE2" => StepKind::Call {
                    to: to()?,
                    expected,
                },
                other => bail!("transaction {n} has unsupported type {other}"),
            };
            Ok(Step { kind, value, data })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn artifacts() {
        assert_eq!(
            init_code(r#"{"abi":[],"bytecode":{"object":"0x6080","sourceMap":""}}"#).unwrap(),
            vec![0x60, 0x80]
        );
        assert_eq!(
            init_code(r#"{"bytecode":"6080"}"#).unwrap(),
            vec![0x60, 0x80]
        );
        assert!(init_code(r#"{"bytecode":{"object":"0x"}}"#).is_err());
        assert!(init_code(r#"{"abi":[]}"#).is_err());
        assert!(
            init_code(r#"{"bytecode":{"object":"0x60__$1234567890abcdef1234567890abcdef12$__"}}"#)
                .is_err()
        );
    }

    const SENDER: &str = "0x32362C314aA26F1401e81985da5b510d3E957A3D";

    fn broadcast(from: &str) -> String {
        format!(
            r#"{{"transactions":[
              {{"hash":null,"transactionType":"CREATE","contractName":"Factory",
                "contractAddress":"0x1111111111111111111111111111111111111111",
                "transaction":{{"from":"{from}","gas":"0x1","value":"0x0","input":"0x6080","nonce":"0x0"}}}},
              {{"transactionType":"CALL","contractAddress":"0x1111111111111111111111111111111111111111",
                "transaction":{{"from":"{from}","to":"0x1111111111111111111111111111111111111111",
                "value":"0x10","input":"0xa9059cbb"}}}},
              {{"transactionType":"CREATE2","contractAddress":"0x2222222222222222222222222222222222222222",
                "transaction":{{"from":"{from}","to":"0x4e59b44847b379578588920ca78fbf26c0b4956c",
                "data":"0x00"}}}}
            ],"receipts":[]}}"#
        )
    }

    #[test]
    fn broadcast_plan() {
        let sender = parse_address(SENDER).unwrap();
        let steps = broadcast_steps(&broadcast(SENDER), sender).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(
            steps[0],
            Step {
                kind: StepKind::Create {
                    expected: Some(H160::repeat_byte(0x11))
                },
                value: 0,
                data: vec![0x60, 0x80],
            }
        );
        assert_eq!(
            steps[1].kind,
            StepKind::Call {
                to: H160::repeat_byte(0x11),
                expected: None
            }
        );
        assert_eq!(steps[1].value, 16);
        assert_eq!(steps[2].data, vec![0]);
        assert!(matches!(
            steps[2].kind,
            StepKind::Call { expected: Some(e), .. } if e == H160::repeat_byte(0x22)
        ));
    }

    #[test]
    fn broadcast_from_another_sender_is_refused() {
        let sender = parse_address(SENDER).unwrap();
        let other = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
        let error = broadcast_steps(&broadcast(other), sender).unwrap_err();
        assert!(error.to_string().contains("--sender"));
        assert!(
            broadcast_steps(
                r#"{"transactions":[{"transactionType":"DELEGATECALL",
            "transaction":{}}]}"#,
                sender
            )
            .is_err()
        );
    }
}
