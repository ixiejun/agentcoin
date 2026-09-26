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
    AccountId, AuraPq, Balance, Block, Executive, InherentDataExt, Nonce, Offences, PqAccounts,
    RandomnessCr, Runtime, RuntimeCall, RuntimeGenesisConfig, System, TransactionPayment, VERSION,
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
            use crate::{AllPalletsWithSystem, PqAccounts};

            let mut list = Vec::<BenchmarkList>::new();
            list_benchmarks!(list, extra);
            (list, AllPalletsWithSystem::storage_info())
        }

        fn dispatch_benchmark(
            config: frame_benchmarking::BenchmarkConfig,
        ) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
            use frame_benchmarking::BenchmarkBatch;
            use frame_support::traits::WhitelistedStorageKeys;
            use crate::{AllPalletsWithSystem, PqAccounts};

            let whitelist = AllPalletsWithSystem::whitelisted_storage_keys();
            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);
            add_benchmarks!(params, batches);
            Ok(batches)
        }
    }

    impl ac_primitives::profile::ChainProfileApi<Block> for Runtime {
        fn profile() -> ac_primitives::ChainProfile {
            ac_primitives::ChainProfile::AGENTCOIN
        }
    }
}
