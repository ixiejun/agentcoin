//! Test harness: genesis from presets, block production and signed transactions.
// Test code: plain arithmetic on test balances and an explicit six-input transaction builder
// keep the scenarios readable; production code keeps the stricter lints.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::too_many_arguments
)]

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, SigAlg};
use ac_runtime::transaction::{
    ChainContext, TxParams, assemble, authorized_extensions, implicit_from, implicit_from_state,
    payload,
};
use ac_runtime::{
    AccountId, Balance, Executive, Header, Runtime, RuntimeCall, RuntimeGenesisConfig, System,
    UncheckedExtrinsic, VERSION,
};
use frame_support::genesis_builder_helper::{build_state, get_preset};
use pallet_pq_accounts::{TX_SIGNING_CONTEXT, derived_account};
use sp_core::H256;
use sp_runtime::generic::Era;
use sp_runtime::traits::Header as _;
use sp_runtime::{ApplyExtrinsicResult, Digest};

pub type Hashing = ac_primitives::Blake3Hasher;

/// Parent hash used for block 1; plays the role of the genesis hash.
pub const GENESIS: H256 = H256([0x77; 32]);

/// Recursively merges a JSON patch into a base document (what chain-spec building does).
fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

/// Full genesis JSON of a named runtime preset.
pub fn preset_json(preset: &str) -> Vec<u8> {
    let patch = get_preset::<RuntimeGenesisConfig>(
        &Some(preset.into()),
        ac_runtime::genesis_config_presets::get_preset,
    )
    .expect("preset exists");
    let mut full = serde_json::to_value(RuntimeGenesisConfig::default()).unwrap();
    merge(&mut full, serde_json::from_slice(&patch).unwrap());
    serde_json::to_vec(&full).unwrap()
}

/// Externalities with the storage of a named runtime preset.
pub fn preset_ext(preset: &str) -> sp_io::TestExternalities {
    let json = preset_json(preset);
    let mut ext = sp_io::TestExternalities::default();
    ext.execute_with(|| build_state::<RuntimeGenesisConfig>(json).unwrap());
    ext
}

/// Dev chain externalities positioned inside block 1.
pub fn dev_ext() -> sp_io::TestExternalities {
    let mut ext = preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET);
    ext.execute_with(|| start_block(1, GENESIS));
    ext
}

pub fn start_block(number: u32, parent: H256) {
    let header = Header::new(
        number,
        Default::default(),
        Default::default(),
        parent,
        Digest::default(),
    );
    Executive::initialize_block(&header);
}

/// Finalizes the current block and starts the next one.
pub fn next_block() {
    let number = System::block_number();
    pallet_timestamp::Pallet::<Runtime>::set_timestamp(
        u64::from(number) * ac_runtime::MILLISECS_PER_BLOCK,
    );
    let header = Executive::finalize_block();
    let hash = header.hash();
    start_block(header.number + 1, hash);
}

/// A signer: key plus the account it controls.
pub struct Signer {
    pub key: SigningKey,
    pub account: AccountId,
}

impl Signer {
    /// A development account (ML-DSA-44).
    pub fn dev(name: &str) -> Self {
        let key =
            SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap();
        let account = derived_account(&key.public_key().unwrap());
        Self { key, account }
    }

    /// A fresh account from a seed byte.
    pub fn fresh(seed: u8, alg: SigAlg) -> Self {
        let key = SigningKey::from_seed(alg, &SecretSeed::new([seed; 32])).unwrap();
        let account = derived_account(&key.public_key().unwrap());
        Self { key, account }
    }

    pub fn public(&self) -> PqPublicKey {
        self.key.public_key().unwrap()
    }
}

pub fn context() -> ChainContext {
    ChainContext {
        genesis_hash: System::block_hash(0),
        spec_version: VERSION.spec_version,
        transaction_version: VERSION.transaction_version,
    }
}

pub fn immortal(nonce: u32) -> TxParams {
    TxParams {
        nonce,
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: System::block_hash(0),
    }
}

/// Builds a signed transaction with explicit control over every input.
pub fn build(
    signer: &Signer,
    key: &SigningKey,
    call: RuntimeCall,
    carry_key: bool,
    params: TxParams,
    context: ChainContext,
) -> UncheckedExtrinsic {
    let extensions = authorized_extensions(&params);
    let implicit = implicit_from(&context, &params);
    let digest = payload(&call, &extensions, &implicit).unwrap();
    let signature = key.sign_deterministic(&digest, TX_SIGNING_CONTEXT).unwrap();
    assemble(
        call,
        signer.account.clone(),
        signature,
        carry_key.then(|| key.public_key().unwrap()),
        extensions,
    )
}

/// Signs `call` for the signer's current nonce; carries the key iff it is unregistered.
pub fn signed(signer: &Signer, call: RuntimeCall) -> UncheckedExtrinsic {
    let nonce = System::account_nonce(&signer.account);
    let first = pallet_pq_accounts::Keys::<Runtime>::get(&signer.account).is_none();
    let params = immortal(nonce);
    // The implicit data computed from chain facts must equal what the chain computes itself.
    let from_state = implicit_from_state(&authorized_extensions(&params)).unwrap();
    assert_eq!(implicit_from(&context(), &params), from_state);
    build(signer, &signer.key, call, first, params, context())
}

pub fn apply(xt: UncheckedExtrinsic) -> ApplyExtrinsicResult {
    Executive::apply_extrinsic(xt)
}

pub fn transfer(to: &AccountId, amount: Balance) -> RuntimeCall {
    RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: to.clone(),
        value: amount,
    })
}

pub fn free(who: &AccountId) -> Balance {
    pallet_balances::Pallet::<Runtime>::free_balance(who)
}

pub fn issuance() -> Balance {
    pallet_balances::Pallet::<Runtime>::total_issuance()
}

/// Sum of every account's free and reserved balance.
pub fn sum_of_balances() -> Balance {
    frame_system::Account::<Runtime>::iter()
        .map(|(_, info)| info.data.free + info.data.reserved)
        .sum()
}

/// Fees (including tips) paid in the current block, from `TransactionFeePaid` events.
pub fn fees_paid() -> Balance {
    System::events()
        .into_iter()
        .filter_map(|r| match r.event {
            ac_runtime::RuntimeEvent::TransactionPayment(
                pallet_transaction_payment::Event::TransactionFeePaid {
                    actual_fee, tip, ..
                },
            ) => Some(actual_fee + tip),
            _ => None,
        })
        .sum()
}
