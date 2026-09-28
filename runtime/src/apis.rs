//! Runtime API implementations.

use alloc::vec::Vec;
use frame_support::genesis_builder_helper::{build_state, get_preset};
use sp_api::impl_runtime_apis;
use sp_core::OpaqueMetadata;
use sp_runtime::{
    ApplyExtrinsicResult,
    traits::Block as BlockT,
    transaction_validity::{TransactionSource, TransactionValidity},
};
use sp_version::RuntimeVersion;

use super::{
    AccountId, AuraPq, Balance, Block, BlockNumber, Emission, EvmSupport, Executive,
    InherentDataExt, Nonce, Offences, PqAccounts, RandomnessCr, Revive, Runtime, RuntimeCall,
    RuntimeGenesisConfig, StakingPos, System, TransactionPayment, TreasuryDual, VERSION,
    ValidatorSet,
};

impl_runtime_apis! {
    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: <Block as BlockT>::LazyBlock) {
            Executive::execute_block(block);
        }

        fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }

        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }

        fn metadata_versions() -> Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }

        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: <Block as BlockT>::LazyBlock,
            data: sp_inherents::InherentData,
        ) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
        fn account_nonce(account: AccountId) -> Nonce {
            System::account_nonce(account)
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            build_state::<RuntimeGenesisConfig>(config)
        }

        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            get_preset::<RuntimeGenesisConfig>(id, crate::genesis_config_presets::get_preset)
        }

        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            crate::genesis_config_presets::preset_names()
        }
    }

    // Required by the node's RPC layer. AgentCoin has no SDK session keys: validator keys are
    // ML-DSA keys managed by Aura-PQ outside the SDK keystore, so there is nothing to generate.
    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(
            _owner: Vec<u8>,
            _seed: Option<Vec<u8>>,
        ) -> sp_session::OpaqueGeneratedSessionKeys {
            sp_session::OpaqueGeneratedSessionKeys { keys: Vec::new(), proof: Vec::new() }
        }

        fn decode_session_keys(
            _encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, sp_core::crypto::KeyTypeId)>> {
            None
        }
    }

    impl ac_primitives::aura_pq::AuraPqApi<Block> for Runtime {
        fn slot_duration() -> u64 {
            AuraPq::slot_duration()
        }

        fn authorities() -> Vec<ac_crypto::PqPublicKey> {
            AuraPq::authorities()
        }
    }

    impl ac_primitives::validator_set::ValidatorSetApi<Block> for Runtime {
        fn authority_set() -> (ac_primitives::ac_bft::SetId, Vec<ac_primitives::ac_bft::Authority>) {
            ValidatorSet::authority_set()
        }

        fn epoch_length() -> u64 {
            ValidatorSet::epoch_length()
        }

        fn historical_set(
            set_id: ac_primitives::ac_bft::SetId,
        ) -> Option<Vec<ac_primitives::ac_bft::Authority>> {
            ValidatorSet::historical_set(set_id)
        }
    }

    impl ac_primitives::staking::StakingApi<Block, AccountId> for Runtime {
        fn stake(who: AccountId) -> ac_primitives::staking::AccountStake {
            StakingPos::stake_of(&who)
        }

        fn candidate(who: AccountId) -> Option<ac_primitives::staking::CandidateInfo<AccountId>> {
            StakingPos::candidate_info(&who)
        }

        fn total_active() -> u128 {
            <StakingPos as ac_primitives::validator_set::StakingInterface>::total_active()
        }

        fn minimums() -> (u128, u128) {
            StakingPos::minimums()
        }

        fn last_election() -> Option<ac_primitives::staking::ElectionInfo<AccountId>> {
            StakingPos::last_election()
        }

        fn transition() -> ac_primitives::staking::TransitionProgress {
            ValidatorSet::transition_progress()
        }
    }

    impl ac_primitives::offences::OffencesApi<Block> for Runtime {
        fn report_extrinsic(
            evidence: ac_primitives::offences::Evidence,
        ) -> Option<sp_runtime::OpaqueExtrinsic> {
            let xt = crate::transaction::report_extrinsic(evidence)?;
            sp_runtime::OpaqueExtrinsic::try_from_encoded_extrinsic(&parity_scale_codec::Encode::encode(&xt)).ok()
        }

        fn offences(
            set_id: ac_primitives::ac_bft::SetId,
        ) -> Vec<(ac_crypto::PqPublicKey, ac_primitives::offences::OffenceKey)> {
            Offences::offences(set_id)
        }
    }

    impl ac_primitives::randomness::RandomnessApi<Block> for Runtime {
        fn latest() -> Option<(u64, sp_core::H256)> {
            RandomnessCr::latest()
        }

        fn random(subject: Vec<u8>) -> Option<(u64, sp_core::H256)> {
            RandomnessCr::random(&subject)
        }

        fn epoch_randomness(epoch: u64) -> Option<sp_core::H256> {
            RandomnessCr::epoch_randomness(epoch)
        }

        fn reveals(epoch: u64) -> Vec<([u8; 32], [u8; 32])> {
            RandomnessCr::reveals(epoch)
        }
    }

    impl ac_primitives::emission::EmissionApi<Block> for Runtime {
        fn epoch_length() -> u64 {
            pallet_emission::EpochLength::<Runtime>::get().unwrap_or(0)
        }

        fn current_epoch() -> ac_primitives::emission::EpochIndex {
            Emission::current_epoch()
        }

        fn scheduled(epoch: ac_primitives::emission::EpochIndex) -> u128 {
            Emission::schedule().map_or(0, |s| s.scheduled(epoch))
        }

        fn reserve() -> u128 {
            pallet_emission::Reserve::<Runtime>::get()
        }

        fn total_minted() -> u128 {
            pallet_emission::TotalMinted::<Runtime>::get()
        }

        fn total_burned() -> u128 {
            Emission::total_burned()
        }
    }

    impl ac_primitives::emission::TreasuryApi<Block, AccountId> for Runtime {
        fn community() -> (AccountId, u128) {
            let who = TreasuryDual::community_account();
            let balance = TreasuryDual::balance(&who);
            (who, balance)
        }

        fn holder() -> (AccountId, u128) {
            let who = TreasuryDual::holder_account();
            let balance = TreasuryDual::balance(&who);
            (who, balance)
        }

        fn floor() -> (AccountId, u128) {
            let who = TreasuryDual::floor_account();
            let balance = TreasuryDual::balance(&who);
            (who, balance)
        }

        fn floor_spendable() -> u128 {
            TreasuryDual::floor_spendable()
        }
    }

    impl pallet_pq_accounts::PqAccountsApi<Block> for Runtime {
        fn current_key(who: AccountId) -> Option<(ac_crypto::PqPublicKey, u32)> {
            PqAccounts::current_key(&who)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }
        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }
        fn query_weight_to_fee(weight: frame_support::weights::Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
        for Runtime
    {
        fn query_call_info(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_call_info(call, len)
        }
        fn query_call_fee_details(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_call_fee_details(call, len)
        }
        fn query_weight_to_fee(weight: frame_support::weights::Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    impl frame_benchmarking::Benchmark<Block> for Runtime {
        fn benchmark_metadata(extra: bool) -> (
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        ) {
            use frame_benchmarking::BenchmarkList;
            use frame_support::traits::StorageInfoTrait;
            use crate::{AllPalletsWithSystem, EvmSupport, Offences, PoaAdmin, PqAccounts, RandomnessCr, StakingPos, ValidatorSet};

            let mut list = Vec::<BenchmarkList>::new();
            list_benchmarks!(list, extra);
            (list, AllPalletsWithSystem::storage_info())
        }

        fn dispatch_benchmark(
            config: frame_benchmarking::BenchmarkConfig,
        ) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
            use frame_benchmarking::BenchmarkBatch;
            use frame_support::traits::WhitelistedStorageKeys;
            use crate::{AllPalletsWithSystem, EvmSupport, Offences, PoaAdmin, PqAccounts, RandomnessCr, StakingPos, ValidatorSet};

            let whitelist = AllPalletsWithSystem::whitelisted_storage_keys();
            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);
            add_benchmarks!(params, batches);
            Ok(batches)
        }
    }

    // Hand-written instead of `pallet_revive::impl_runtime_apis_plus_revive_traits!`, which
    // needs the Ethereum transaction wrapper this chain does not have (m4-evm design D11):
    // every dry run makes the queried origin the payer (design D5), PolkaVM code and
    // secp256k1-signed payloads are refused, and tracing is disabled.
    impl pallet_revive::ReviveApi<Block, AccountId, Balance, Nonce, BlockNumber, u64> for Runtime {
        fn eth_block() -> pallet_revive::EthBlock {
            Revive::eth_block()
        }

        fn eth_block_hash(number: pallet_revive::U256) -> Option<pallet_revive::H256> {
            Revive::eth_block_hash_from_number(number)
        }

        fn eth_receipt_data() -> Vec<pallet_revive::ReceiptGasInfo> {
            Revive::eth_receipt_data()
        }

        fn block_gas_limit() -> pallet_revive::U256 {
            Revive::evm_block_gas_limit()
        }

        fn max_extrinsic_weight_in_gas() -> pallet_revive::U256 {
            Revive::evm_max_extrinsic_weight_in_gas()
        }

        fn balance(address: pallet_revive::H160) -> pallet_revive::U256 {
            Revive::evm_balance(&address)
        }

        fn gas_price() -> pallet_revive::U256 {
            Revive::evm_base_fee()
        }

        fn nonce(address: pallet_revive::H160) -> Nonce {
            System::account_nonce(revive_account_id(&address))
        }

        fn call(
            origin: AccountId,
            dest: pallet_revive::H160,
            value: Balance,
            weight_limit: Option<frame_support::weights::Weight>,
            storage_deposit_limit: Option<Balance>,
            input_data: Vec<u8>,
        ) -> pallet_revive::ContractResult<pallet_revive::ExecReturnValue, Balance> {
            Revive::prepare_dry_run(&origin);
            EvmSupport::with_payer(origin.clone(), || {
                Revive::bare_call(
                    crate::RuntimeOrigin::signed(origin),
                    dest,
                    Revive::convert_native_to_evm(value),
                    dry_run_limits(weight_limit, storage_deposit_limit),
                    input_data,
                    &pallet_revive::ExecConfig::new_substrate_tx().with_dry_run(Default::default()),
                )
            })
        }

        fn instantiate(
            origin: AccountId,
            value: Balance,
            weight_limit: Option<frame_support::weights::Weight>,
            storage_deposit_limit: Option<Balance>,
            code: pallet_revive::Code,
            data: Vec<u8>,
            salt: Option<[u8; 32]>,
        ) -> pallet_revive::ContractResult<pallet_revive::InstantiateReturnValue, Balance> {
            // Only EVM init code, exactly like the call filter (design D3).
            let evm_code = matches!(
                &code,
                pallet_revive::Code::Upload(bytes)
                    if !bytes.starts_with(&ac_primitives::evm::POLKAVM_MAGIC)
            );
            if !evm_code {
                return pallet_revive::ContractResult {
                    result: Err(pallet_revive::Error::<Runtime>::CodeRejected.into()),
                    ..Default::default()
                };
            }
            Revive::prepare_dry_run(&origin);
            EvmSupport::with_payer(origin.clone(), || {
                Revive::bare_instantiate(
                    crate::RuntimeOrigin::signed(origin),
                    Revive::convert_native_to_evm(value),
                    dry_run_limits(weight_limit, storage_deposit_limit),
                    code,
                    data,
                    salt,
                    &pallet_revive::ExecConfig::new_substrate_tx().with_dry_run(Default::default()),
                )
            })
        }

        fn eth_transact(
            tx: pallet_revive::evm::GenericTransaction,
        ) -> Result<pallet_revive::EthTransactInfo<Balance>, pallet_revive::EthTransactError> {
            let payer = revive_account_id(&tx.from.unwrap_or_default());
            EvmSupport::with_payer(payer, || Revive::dry_run_eth_transact(tx, Default::default()))
        }

        fn eth_transact_with_config(
            tx: pallet_revive::evm::GenericTransaction,
            config: pallet_revive::DryRunConfig<u64>,
        ) -> Result<pallet_revive::EthTransactInfo<Balance>, pallet_revive::EthTransactError> {
            let payer = revive_account_id(&tx.from.unwrap_or_default());
            EvmSupport::with_payer(payer, || Revive::dry_run_eth_transact(tx, config))
        }

        fn eth_estimate_gas(
            tx: pallet_revive::evm::GenericTransaction,
            config: pallet_revive::DryRunConfig<u64>,
        ) -> Result<pallet_revive::U256, pallet_revive::EthTransactError> {
            let payer = revive_account_id(&tx.from.unwrap_or_default());
            EvmSupport::with_payer(payer, || Revive::eth_estimate_gas(tx, config))
        }

        // Ethereum (secp256k1) transactions are never accepted (red line 1, D13).
        fn eth_pre_dispatch_weight(
            _tx: Vec<u8>,
        ) -> Result<frame_support::weights::Weight, pallet_revive::EthTransactError> {
            Err(pallet_revive::EthTransactError::Message(
                "Ethereum-signed transactions are not supported".into(),
            ))
        }

        // Only PolkaVM code is uploaded on its own; EVM runtime code comes from init code.
        fn upload_code(
            _origin: AccountId,
            _code: Vec<u8>,
            _storage_deposit_limit: Option<Balance>,
        ) -> pallet_revive::CodeUploadResult<Balance> {
            Err(pallet_revive::Error::<Runtime>::CodeRejected.into())
        }

        fn get_storage(address: pallet_revive::H160, key: [u8; 32]) -> pallet_revive::GetStorageResult {
            Revive::get_storage(address, key)
        }

        fn get_storage_var_key(
            address: pallet_revive::H160,
            key: Vec<u8>,
        ) -> pallet_revive::GetStorageResult {
            Revive::get_storage_var_key(address, key)
        }

        // Tracing is not offered (non-goal of m4-evm).
        fn trace_block(
            _block: Block,
            _config: pallet_revive::evm::TracerType,
        ) -> Vec<(u32, pallet_revive::evm::Trace)> {
            Vec::new()
        }

        fn trace_tx(
            _block: Block,
            _tx_index: u32,
            _config: pallet_revive::evm::TracerType,
        ) -> Option<pallet_revive::evm::Trace> {
            None
        }

        fn trace_call(
            _tx: pallet_revive::evm::GenericTransaction,
            _config: pallet_revive::evm::TracerType,
        ) -> Result<pallet_revive::evm::Trace, pallet_revive::EthTransactError> {
            Err(pallet_revive::EthTransactError::Message("tracing is not supported".into()))
        }

        fn trace_call_with_config(
            _tx: pallet_revive::evm::GenericTransaction,
            _tracer_type: pallet_revive::evm::TracerType,
            _config: pallet_revive::evm::TracingConfig,
        ) -> Result<pallet_revive::evm::Trace, pallet_revive::EthTransactError> {
            Err(pallet_revive::EthTransactError::Message("tracing is not supported".into()))
        }

        fn block_author() -> pallet_revive::H160 {
            Revive::block_author()
        }

        fn address(account_id: AccountId) -> pallet_revive::H160 {
            use pallet_revive::AddressMapper;
            <Runtime as pallet_revive::Config>::AddressMapper::to_address(&account_id)
        }

        fn account_id(address: pallet_revive::H160) -> AccountId {
            revive_account_id(&address)
        }

        fn runtime_pallets_address() -> pallet_revive::H160 {
            pallet_revive::RUNTIME_PALLETS_ADDR
        }

        fn code(address: pallet_revive::H160) -> Vec<u8> {
            Revive::code(&address)
        }

        fn new_balance_with_dust(
            balance: pallet_revive::U256,
        ) -> Result<(Balance, u32), pallet_revive::BalanceConversionError> {
            Revive::new_balance_with_dust(balance)
        }
    }

    impl ac_primitives::profile::ChainProfileApi<Block> for Runtime {
        fn profile() -> ac_primitives::ChainProfile {
            ac_primitives::ChainProfile::AGENTCOIN
        }
    }
}

/// The account behind an EVM address (its mapped account, or its fallback account).
fn revive_account_id(address: &pallet_revive::H160) -> AccountId {
    use pallet_revive::AddressMapper;
    <Runtime as pallet_revive::Config>::AddressMapper::to_account_id(address)
}

/// Limits of a dry run: the given ones, or a whole block and an unlimited deposit.
fn dry_run_limits(
    weight_limit: Option<frame_support::weights::Weight>,
    storage_deposit_limit: Option<Balance>,
) -> pallet_revive::TransactionLimits<Runtime> {
    let block: frame_system::limits::BlockWeights =
        <Runtime as frame_system::Config>::BlockWeights::get();
    pallet_revive::TransactionLimits::WeightAndDeposit {
        weight_limit: weight_limit.unwrap_or(block.max_block),
        deposit_limit: storage_deposit_limit.unwrap_or(Balance::MAX),
    }
}

/// Required by revive's Ethereum dry runs; only the Ethereum-context calls (never dispatched on
/// this chain, design D3) carry a weight limit to replace.
impl pallet_revive::evm::runtime::SetWeightLimit for RuntimeCall {
    fn set_weight_limit(
        &mut self,
        new_weight_limit: frame_support::weights::Weight,
    ) -> frame_support::weights::Weight {
        match self {
            RuntimeCall::Revive(
                pallet_revive::Call::eth_call { weight_limit, .. }
                | pallet_revive::Call::eth_instantiate_with_code { weight_limit, .. },
            ) => core::mem::replace(weight_limit, new_weight_limit),
            _ => frame_support::weights::Weight::default(),
        }
    }
}
