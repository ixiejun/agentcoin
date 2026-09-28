//! Benchmarks included in the runtime (run with `frame-omni-bencher`).

frame_benchmarking::define_benchmarks!(
    [pallet_pq_accounts, PqAccounts]
    [pallet_validator_set, ValidatorSet]
    [pallet_ac_offences, Offences]
    [pallet_randomness_cr, RandomnessCr]
    [pallet_emission, Emission]
    [pallet_treasury_dual, TreasuryDual]
    [pallet_poa_admin, PoaAdmin]
    [pallet_staking_pos, StakingPos]
    [pallet_evm_support, EvmSupport]
    [pallet_revive, Revive]
);
