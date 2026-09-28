//! The wallet as an external signer for EVM contracts (m4-evm, spec clients/wallet-cli
//! "钱包作为 EVM 外部签名器", design D10).
//!
//! Contract transactions are ordinary AgentCoin extrinsics (`Revive::call`,
//! `Revive::instantiate_with_code`) signed with the account's ML-DSA key. Before signing, a dry
//! run on the latest state sizes the weight and storage-deposit limits and refuses calls that
//! would revert.

pub mod abi;
pub mod foundry;

use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use ac_runtime::{Runtime, RuntimeCall, RuntimeEvent, UncheckedExtrinsic};
use alloy_core::primitives::Address;
use anyhow::{Context, Result, bail};
use pallet_revive::{AddressMapper, H160, StorageDeposit};
use sp_core::H256;
use sp_runtime::{AccountId32, Weight};

use crate::client::{Inclusion, NodeClient, transaction_hash};
use crate::ops::{INCLUSION_TIMEOUT, sign_call};
use crate::wallet::Wallet;
use foundry::{Step, StepKind};

/// Extra weight over the dry run's requirement, in percent (design D10).
pub const WEIGHT_MARGIN_PERCENT: u64 = 20;
/// Extra storage deposit over the dry run's peak, in percent (design D10).
pub const DEPOSIT_MARGIN_PERCENT: u128 = 10;

/// The EVM address of an account: `keccak256(account)[12..]`, the runtime's mapping.
#[must_use]
pub fn evm_address(account: &AccountId32) -> H160 {
    <Runtime as pallet_revive::Config>::AddressMapper::to_address(account)
}

/// The EIP-55 mixed-case form of `address`.
#[must_use]
pub fn checksummed(address: H160) -> String {
    Address::from(address.0).to_checksum(None)
}

/// Parses a 20-byte hexadecimal address. Mixed-case input must carry a valid EIP-55 checksum;
/// all-lowercase and all-uppercase input carries none and is accepted as is.
///
/// # Errors
///
/// Malformed addresses and wrong checksums.
pub fn parse_address(text: &str) -> Result<H160> {
    let text = text.trim();
    let digits = text.strip_prefix("0x").unwrap_or(text);
    let has_lower = digits.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = digits.chars().any(|c| c.is_ascii_uppercase());
    let address = if has_lower && has_upper {
        Address::parse_checksummed(format!("0x{digits}"), None)
            .map_err(|_| anyhow::anyhow!("{text} has a wrong EIP-55 checksum"))?
    } else {
        let bytes = hex::decode(digits).with_context(|| format!("invalid address {text}"))?;
        if bytes.len() != 20 {
            bail!("an EVM address has 20 bytes: {text}");
        }
        Address::from_slice(&bytes)
    };
    Ok(H160(address.0.0))
}

/// A contract transaction before sizing and signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractTx {
    /// Call `to` with `data`.
    Call {
        /// Callee.
        to: H160,
        /// Value in the smallest unit.
        value: u128,
        /// Call data.
        data: Vec<u8>,
    },
    /// Deploy EVM init code (constructor arguments appended).
    Deploy {
        /// Value in the smallest unit.
        value: u128,
        /// Init code.
        code: Vec<u8>,
    },
}

/// Weight and storage-deposit limits of a contract transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Weight limit.
    pub weight: Weight,
    /// Storage deposit limit in the smallest unit.
    pub deposit: u128,
}

/// User choices that override the dry run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// Ref-time limit instead of the estimate.
    pub ref_time: Option<u64>,
    /// Proof-size limit instead of the estimate.
    pub proof_size: Option<u64>,
    /// Storage deposit limit instead of the estimate.
    pub deposit: Option<u128>,
    /// Sign even if the dry run reverts or fails.
    pub force: bool,
}

fn with_percent(value: u64, percent: u64) -> u64 {
    // Rounded up, so the margin never shrinks below the requirement.
    value.saturating_add(value.saturating_mul(percent).div_ceil(100))
}

/// Limits from a dry run: the required weight plus [`WEIGHT_MARGIN_PERCENT`], the peak storage
/// deposit plus [`DEPOSIT_MARGIN_PERCENT`], then any overrides.
#[must_use]
pub fn limits(
    required: Weight,
    peak_deposit: &StorageDeposit<u128>,
    overrides: &Overrides,
) -> Limits {
    let deposit = match peak_deposit {
        StorageDeposit::Charge(amount) => {
            amount.saturating_add(amount.saturating_mul(DEPOSIT_MARGIN_PERCENT).div_ceil(100))
        }
        StorageDeposit::Refund(_) => 0,
    };
    Limits {
        weight: Weight::from_parts(
            overrides
                .ref_time
                .unwrap_or_else(|| with_percent(required.ref_time(), WEIGHT_MARGIN_PERCENT)),
            overrides
                .proof_size
                .unwrap_or_else(|| with_percent(required.proof_size(), WEIGHT_MARGIN_PERCENT)),
        ),
        deposit: overrides.deposit.unwrap_or(deposit),
    }
}

/// The runtime call for `tx` under `limits`.
#[must_use]
pub fn runtime_call(tx: &ContractTx, limits: Limits) -> RuntimeCall {
    match tx {
        ContractTx::Call { to, value, data } => RuntimeCall::Revive(pallet_revive::Call::call {
            dest: *to,
            value: *value,
            weight_limit: limits.weight,
            storage_deposit_limit: limits.deposit,
            data: data.clone(),
        }),
        ContractTx::Deploy { value, code } => {
            RuntimeCall::Revive(pallet_revive::Call::instantiate_with_code {
                value: *value,
                weight_limit: limits.weight,
                storage_deposit_limit: limits.deposit,
                code: code.clone(),
                // EVM constructors read their arguments from the end of the init code.
                data: Vec::new(),
                salt: None,
            })
        }
    }
}

/// What the dry run said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Estimate {
    /// Limits the transaction is signed with.
    pub limits: Limits,
    /// `None` when the dry run succeeded, otherwise why it would revert or fail.
    pub failure: Option<String>,
    /// Address a deployment would create (for deployments that succeed).
    pub address: Option<H160>,
}

/// Dry-runs `tx` from `origin` on the latest state.
///
/// # Errors
///
/// RPC failures.
pub async fn estimate(
    client: &NodeClient,
    origin: &AccountId32,
    tx: &ContractTx,
    overrides: &Overrides,
) -> Result<Estimate> {
    let (required, peak, outcome) = match tx {
        ContractTx::Call { to, value, data } => {
            let r = client.dry_call(origin, *to, *value, data.clone()).await?;
            let outcome = r.result.map(|out| {
                let failure = out.did_revert().then(|| abi::revert_reason(&out.data));
                (failure, None)
            });
            (r.weight_required, r.max_storage_deposit, outcome)
        }
        ContractTx::Deploy { value, code } => {
            let r = client.dry_deploy(origin, *value, code.clone()).await?;
            let outcome = r.result.map(|out| {
                let failure = out
                    .result
                    .did_revert()
                    .then(|| abi::revert_reason(&out.result.data));
                (failure, Some(out.addr))
            });
            (r.weight_required, r.max_storage_deposit, outcome)
        }
    };
    let (failure, address) = match outcome {
        Ok((failure, address)) => {
            let address = if failure.is_none() { address } else { None };
            (failure, address)
        }
        Err(error) => (Some(format!("failed: {error:?}")), None),
    };
    Ok(Estimate {
        limits: limits(required, &peak, overrides),
        failure,
        address,
    })
}

/// A signed contract transaction.
pub struct Signed {
    /// The extrinsic.
    pub xt: UncheckedExtrinsic,
    /// Its hash (the node's and the eth-RPC adapter's).
    pub hash: H256,
    /// The dry run it was sized by.
    pub estimate: Estimate,
}

/// Dry-runs, sizes and signs `tx`. A dry run that reverts or fails stops here unless
/// `overrides.force`.
///
/// # Errors
///
/// The revert or failure reason, wrong password, or RPC failures.
pub async fn prepare(
    client: &NodeClient,
    wallet: &Wallet,
    password: &[u8],
    tx: &ContractTx,
    overrides: &Overrides,
) -> Result<Signed> {
    let estimate = estimate(client, &wallet.account()?, tx, overrides).await?;
    if let Some(failure) = &estimate.failure
        && !overrides.force
    {
        bail!("the dry run {failure}; nothing was submitted (use --force to submit anyway)");
    }
    let xt = sign_call(client, wallet, password, runtime_call(tx, estimate.limits)).await?;
    Ok(Signed {
        hash: transaction_hash(&xt),
        xt,
        estimate,
    })
}

/// A submitted and included contract transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Transaction hash.
    pub hash: H256,
    /// Where it was included and whether it succeeded.
    pub inclusion: Inclusion,
    /// Contract a deployment created.
    pub contract: Option<H160>,
    /// Why it failed on chain.
    pub error: Option<String>,
}

/// Submits a signed transaction, waits for inclusion and reads its outcome.
///
/// # Errors
///
/// Rejection by the node, RPC failures or the inclusion timeout.
pub async fn submit(client: &NodeClient, signed: &Signed, deployer: H160) -> Result<Outcome> {
    let inclusion = client
        .submit_and_watch(&signed.xt, INCLUSION_TIMEOUT)
        .await?;
    let events = client
        .extrinsic_events(inclusion.block_hash, inclusion.index)
        .await?;
    let contract = events.iter().find_map(|event| match event {
        RuntimeEvent::Revive(pallet_revive::Event::Instantiated {
            deployer: d,
            contract,
        }) if *d == deployer => Some(*contract),
        _ => None,
    });
    let error = events.iter().find_map(|event| match event {
        RuntimeEvent::System(frame_system::Event::ExtrinsicFailed { dispatch_error, .. }) => {
            Some(format!("{dispatch_error:?}"))
        }
        _ => None,
    });
    Ok(Outcome {
        hash: signed.hash,
        inclusion,
        contract,
        error,
    })
}

/// Why a broadcast stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Transaction `step` (1-based) failed on chain or in its dry run.
    Failed {
        /// 1-based position.
        step: usize,
        /// Reason.
        reason: String,
    },
    /// Transaction `step` created `actual` where the script predicted `expected`.
    AddressMismatch {
        /// 1-based position.
        step: usize,
        /// Predicted address.
        expected: H160,
        /// Address on chain (`None`: nothing was created there).
        actual: Option<H160>,
    },
}

impl core::fmt::Display for Stop {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Failed { step, reason } => write!(f, "transaction {step} failed: {reason}"),
            Self::AddressMismatch {
                step,
                expected,
                actual,
            } => {
                let actual = actual.map_or_else(|| "nothing".to_owned(), checksummed);
                write!(
                    f,
                    "transaction {step} created {actual}, but the script predicted {}; \
                     stopped, later transactions were not sent",
                    checksummed(*expected)
                )
            }
        }
    }
}

/// What executing one step reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    /// Transaction hash.
    pub hash: H256,
    /// Whether it succeeded on chain.
    pub success: bool,
    /// Failure reason.
    pub error: Option<String>,
    /// Contract created by a `CREATE` step.
    pub created: Option<H160>,
    /// Whether code exists at a `CREATE2` step's predicted address afterwards.
    pub expected_has_code: bool,
}

/// Runs broadcast `steps` in order through `execute` (which submits one step and waits for its
/// inclusion). Stops at the first failure or address mismatch; later steps are never executed.
///
/// # Errors
///
/// Errors of `execute` (node or RPC failures).
pub async fn run_broadcast(
    steps: &[Step],
    mut execute: impl AsyncFnMut(&Step) -> Result<StepResult>,
) -> Result<(Vec<StepResult>, Option<Stop>)> {
    let mut done = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i.saturating_add(1);
        let result = match execute(step).await {
            Ok(result) => result,
            Err(error) => {
                return Ok((
                    done,
                    Some(Stop::Failed {
                        step: n,
                        reason: format!("{error:#}"),
                    }),
                ));
            }
        };
        let stop = if !result.success {
            Some(Stop::Failed {
                step: n,
                reason: result.error.clone().unwrap_or_else(|| "failed".to_owned()),
            })
        } else {
            match step.kind {
                StepKind::Create {
                    expected: Some(expected),
                } if result.created != Some(expected) => Some(Stop::AddressMismatch {
                    step: n,
                    expected,
                    actual: result.created,
                }),
                StepKind::Call {
                    expected: Some(expected),
                    ..
                } if !result.expected_has_code => Some(Stop::AddressMismatch {
                    step: n,
                    expected,
                    actual: None,
                }),
                _ => None,
            }
        };
        done.push(result);
        if stop.is_some() {
            return Ok((done, stop));
        }
    }
    Ok((done, None))
}

/// The transaction of a broadcast step.
#[must_use]
pub fn step_tx(step: &Step) -> ContractTx {
    match step.kind {
        StepKind::Create { .. } => ContractTx::Deploy {
            value: step.value,
            code: step.data.clone(),
        },
        StepKind::Call { to, .. } => ContractTx::Call {
            to,
            value: step.value,
            data: step.data.clone(),
        },
    }
}

/// Who signs and where transactions go.
#[derive(Clone, Copy)]
pub struct Sender<'a> {
    /// Node connection.
    pub client: &'a NodeClient,
    /// The wallet.
    pub wallet: &'a Wallet,
    /// Its passphrase.
    pub password: &'a [u8],
}

/// Runs a `forge script` broadcast from the wallet: each step is dry-run, signed, submitted and
/// awaited before the next; deployments are checked against the script's predicted addresses.
/// `report` sees each step's result as it lands.
///
/// # Errors
///
/// Only node or RPC failures outside a step; a failing step is reported as a [`Stop`].
pub async fn broadcast(
    sender: Sender<'_>,
    steps: &[Step],
    overrides: &Overrides,
    mut report: impl FnMut(usize, &StepResult),
) -> Result<(Vec<StepResult>, Option<Stop>)> {
    let Sender {
        client,
        wallet,
        password,
    } = sender;
    let me = evm_address(&wallet.account()?);
    let mut n = 0usize;
    run_broadcast(steps, async |step: &Step| {
        n = n.saturating_add(1);
        let signed = prepare(client, wallet, password, &step_tx(step), overrides).await?;
        let outcome = submit(client, &signed, me).await?;
        let expected_has_code = match step.kind {
            StepKind::Call {
                expected: Some(expected),
                ..
            } => !client.evm_code(expected).await?.is_empty(),
            _ => false,
        };
        let result = StepResult {
            hash: outcome.hash,
            success: outcome.inclusion.success,
            error: outcome.error,
            created: outcome.contract,
            expected_has_code,
        };
        report(n, &result);
        Ok(result)
    })
    .await
}

/// A deployment of `init_code` with constructor arguments: either `constructor` (a signature
/// such as `constructor(string,uint256)`) with `args` as text, or `encoded` (ABI-encoded, e.g.
/// from `cast abi-encode`), or none.
///
/// # Errors
///
/// Arguments that do not match the constructor, or invalid hex.
pub fn deployment(
    mut init_code: Vec<u8>,
    constructor: Option<&str>,
    args: &[String],
    encoded: Option<&str>,
    value: u128,
) -> Result<ContractTx> {
    match (constructor, encoded) {
        (Some(signature), None) => {
            init_code.extend(abi::Signature::parse(signature)?.encode_params(args)?);
        }
        (None, Some(encoded)) => {
            if !args.is_empty() {
                bail!("give constructor arguments either as text with --constructor or encoded");
            }
            init_code.extend(abi::parse_hex(encoded)?);
        }
        (None, None) if args.is_empty() => {}
        (None, None) => bail!("constructor arguments need --constructor <signature>"),
        (Some(_), Some(_)) => bail!("use either --constructor or --constructor-args"),
    }
    Ok(ContractTx::Deploy {
        value,
        code: init_code,
    })
}

/// A call of `to` with either `signature` and `args` as text, or raw `data`.
///
/// # Errors
///
/// Invalid addresses, arguments or hex.
pub fn call(
    to: &str,
    signature: Option<&str>,
    args: &[String],
    data: Option<&str>,
    value: u128,
) -> Result<ContractTx> {
    let data = match (signature, data) {
        (Some(signature), None) => abi::Signature::parse(signature)?.calldata(args)?,
        (None, Some(data)) if args.is_empty() => abi::parse_hex(data)?,
        (None, Some(_)) => bail!("arguments need a function signature (--sig)"),
        (None, None) => bail!("give a function signature (--sig) or call data (--data)"),
        (Some(_), Some(_)) => bail!("use either --sig or --data"),
    };
    Ok(ContractTx::Call {
        to: parse_address(to)?,
        value,
        data,
    })
}

/// A signature for the `pq_verify` precompile.
pub struct MessageSignature {
    /// Algorithm (its AlgId is the precompile's `alg`).
    pub alg: SigAlg,
    /// Public key (raw bytes are the precompile's `publicKey`).
    pub public_key: PqPublicKey,
    /// Signature (raw bytes are the precompile's `signature`).
    pub signature: PqSignature,
}

/// Signs `message` with the wallet's current key under the context `agentcoin/evm-verify/v1`,
/// the only context the `pq_verify` precompile accepts.
///
/// # Errors
///
/// Wrong password or signing failures.
pub fn sign_message(wallet: &Wallet, password: &[u8], message: &[u8]) -> Result<MessageSignature> {
    let key = wallet.current_key(password)?;
    let mut rng = ac_crypto::OsRng::new()?;
    let signature = key.sign(message, ac_primitives::evm::EVM_VERIFY_CONTEXT, &mut rng)?;
    Ok(MessageSignature {
        alg: wallet.current().0,
        public_key: key.public_key()?,
        signature,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use foundry::StepKind;

    // Task 6.1 with the address vectors of task 2.8 (independent Keccak-256).
    #[test]
    fn address_vectors() {
        let vectors: [([u8; 32], &str); 2] = [
            ([0x00; 32], "0x88386fc84ba6bc95484008f6362f93160ef3e563"),
            ([0xff; 32], "0xab758a3376d22aedc6a55823d1b3ecbee81b8fb9"),
        ];
        for (account, expected) in vectors {
            let address = evm_address(&AccountId32::new(account));
            assert_eq!(checksummed(address).to_lowercase(), expected);
            assert_eq!(parse_address(&checksummed(address)).unwrap(), address);
        }
    }

    // EIP-55: the examples of the EIP (checked against `cast to-check-sum-address`).
    #[test]
    fn eip55_checksums() {
        for text in [
            "0x52908400098527886E0F7030069857D2E4169EE7",
            "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
            "0xde709f2102306220921060314715629080e2fb77",
            "0x27b1fdb04752bbc536007a920d24acb045561c26",
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ] {
            let address = parse_address(text).unwrap();
            assert_eq!(checksummed(address), text);
            assert_eq!(parse_address(&text.to_lowercase()).unwrap(), address);
        }
        // One flipped letter breaks a mixed-case checksum.
        assert!(parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD").is_err());
        assert!(parse_address("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed00").is_err());
        assert!(parse_address("0xzz").is_err());
    }

    #[test]
    fn transactions_from_arguments() {
        let args = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let ctor = include_str!("vectors/constructor.hex").trim();
        let expected = [vec![0x60, 0x80], abi::parse_hex(ctor).unwrap()].concat();
        let text = deployment(
            vec![0x60, 0x80],
            Some("constructor(string,string,uint256)"),
            &args(&["Token", "TKN", "1000000000000000000000"]),
            None,
            0,
        )
        .unwrap();
        let encoded = deployment(vec![0x60, 0x80], None, &[], Some(ctor), 0).unwrap();
        assert_eq!(text, encoded);
        assert_eq!(
            text,
            ContractTx::Deploy {
                value: 0,
                code: expected
            }
        );
        assert!(deployment(vec![0x60], None, &args(&["1"]), None, 0).is_err());
        assert!(deployment(vec![0x60], Some("(uint256)"), &args(&["1"]), Some("0x"), 0).is_err());

        let to = "0x00000000000000000000000000000000000000aa";
        let call_text = call(
            to,
            Some("transfer(address,uint256)"),
            &args(&[to, "1000"]),
            None,
            5,
        )
        .unwrap();
        let call_data = call(
            to,
            None,
            &[],
            Some(include_str!("vectors/transfer.hex").trim()),
            5,
        )
        .unwrap();
        assert_eq!(call_text, call_data);
        assert!(call(to, None, &[], None, 0).is_err());
        assert!(call(to, None, &args(&["1"]), Some("0x"), 0).is_err());
        assert!(
            call(
                "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD",
                None,
                &[],
                Some("0x"),
                0
            )
            .is_err()
        );
    }

    // Task 6.5: the output verifies under agentcoin/evm-verify/v1, and only there.
    #[test]
    fn message_signatures_verify_in_the_evm_context() {
        let created = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap();
        let signed = sign_message(&created.wallet, b"pw", b"hello").unwrap();
        assert_eq!(signed.alg, SigAlg::MlDsa44);
        let (pk, sig) = (&signed.public_key, &signed.signature);
        let raw_pk = PqPublicKey::new(signed.alg, pk.as_bytes()).unwrap();
        let raw_sig = PqSignature::new(signed.alg, sig.as_bytes()).unwrap();
        ac_crypto::sig::verify(
            &raw_pk,
            b"hello",
            ac_primitives::evm::EVM_VERIFY_CONTEXT,
            &raw_sig,
        )
        .unwrap();
        assert!(
            ac_crypto::sig::verify(
                &raw_pk,
                b"hellO",
                ac_primitives::evm::EVM_VERIFY_CONTEXT,
                &raw_sig
            )
            .is_err()
        );
        assert!(
            ac_crypto::sig::verify(
                &raw_pk,
                b"hello",
                pallet_pq_accounts::TX_SIGNING_CONTEXT,
                &raw_sig
            )
            .is_err()
        );
        assert!(sign_message(&created.wallet, b"wrong", b"hello").is_err());
    }

    #[test]
    fn limits_add_the_margins() {
        let none = Overrides::default();
        let l = limits(
            Weight::from_parts(1_000, 11),
            &StorageDeposit::Charge(1_001),
            &none,
        );
        assert_eq!(l.weight, Weight::from_parts(1_200, 14));
        assert_eq!(l.deposit, 1_102);
        let refund = limits(Weight::zero(), &StorageDeposit::Refund(5), &none);
        assert_eq!((refund.weight, refund.deposit), (Weight::zero(), 0));
        let saturated = limits(
            Weight::from_parts(u64::MAX, u64::MAX),
            &StorageDeposit::Charge(u128::MAX),
            &none,
        );
        assert_eq!(saturated.weight, Weight::from_parts(u64::MAX, u64::MAX));
        assert_eq!(saturated.deposit, u128::MAX);
        let chosen = Overrides {
            ref_time: Some(7),
            proof_size: Some(8),
            deposit: Some(9),
            force: false,
        };
        let l = limits(
            Weight::from_parts(1_000, 11),
            &StorageDeposit::Charge(1),
            &chosen,
        );
        assert_eq!((l.weight, l.deposit), (Weight::from_parts(7, 8), 9));
    }

    #[test]
    fn calls_are_contract_transactions() {
        let limits = Limits {
            weight: Weight::from_parts(1, 2),
            deposit: 3,
        };
        for tx in [
            ContractTx::Call {
                to: H160::repeat_byte(1),
                value: 4,
                data: vec![5],
            },
            ContractTx::Deploy {
                value: 0,
                code: vec![0x60, 0x80],
            },
        ] {
            let encoded = parity_scale_codec::Encode::encode(&runtime_call(&tx, limits));
            assert!(
                ac_primitives::evm::classify_call(&encoded).is_ok(),
                "{tx:?}"
            );
        }
    }

    fn create(expected: u8) -> Step {
        Step {
            kind: StepKind::Create {
                expected: Some(H160::repeat_byte(expected)),
            },
            value: 0,
            data: vec![0x60],
        }
    }

    fn ok(created: Option<u8>) -> StepResult {
        StepResult {
            hash: H256::zero(),
            success: true,
            error: None,
            created: created.map(H160::repeat_byte),
            expected_has_code: false,
        }
    }

    // Scenario "broadcast 地址不一致时中止": the second deployment lands elsewhere; the third
    // transaction is never sent.
    #[tokio::test]
    async fn broadcast_stops_at_an_address_mismatch() {
        let steps = [create(1), create(2), create(3)];
        let mut sent = 0;
        let (done, stop) = run_broadcast(&steps, async |step: &Step| {
            sent += 1;
            let StepKind::Create { expected } = step.kind else {
                return Ok(ok(None));
            };
            let created = expected.map(|e| if e == H160::repeat_byte(2) { 9 } else { e.0[0] });
            Ok(ok(created))
        })
        .await
        .unwrap();
        assert_eq!(sent, 2);
        assert_eq!(done.len(), 2);
        let stop = stop.unwrap();
        assert_eq!(
            stop,
            Stop::AddressMismatch {
                step: 2,
                expected: H160::repeat_byte(2),
                actual: Some(H160::repeat_byte(9)),
            }
        );
        let text = stop.to_string();
        assert!(text.contains(&checksummed(H160::repeat_byte(2))));
        assert!(text.contains(&checksummed(H160::repeat_byte(9))));
    }

    #[tokio::test]
    async fn broadcast_stops_at_a_failure_and_checks_create2() {
        let factory_call = Step {
            kind: StepKind::Call {
                to: H160::repeat_byte(7),
                expected: Some(H160::repeat_byte(8)),
            },
            value: 0,
            data: vec![],
        };
        let (done, stop) = run_broadcast(&[factory_call.clone(), create(1)], async |_: &Step| {
            Ok(ok(None))
        })
        .await
        .unwrap();
        assert_eq!(done.len(), 1);
        assert!(matches!(
            stop,
            Some(Stop::AddressMismatch {
                step: 1,
                actual: None,
                ..
            })
        ));

        let (done, stop) = run_broadcast(&[create(1), create(2)], async |_: &Step| {
            Ok(StepResult {
                success: false,
                error: Some("out of weight".to_owned()),
                ..ok(None)
            })
        })
        .await
        .unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(
            stop,
            Some(Stop::Failed {
                step: 1,
                reason: "out of weight".to_owned()
            })
        );

        let (done, stop) = run_broadcast(&[create(1)], async |_: &Step| {
            anyhow::bail!("the dry run reverted: no")
        })
        .await
        .unwrap();
        assert!(done.is_empty());
        assert!(matches!(stop, Some(Stop::Failed { step: 1, .. })));

        let (done, stop) = run_broadcast(&[create(1), create(2)], async |step: &Step| {
            let StepKind::Create { expected } = step.kind else {
                return Ok(ok(None));
            };
            Ok(ok(expected.map(|e| e.0[0])))
        })
        .await
        .unwrap();
        assert_eq!((done.len(), stop), (2, None));
    }
}
