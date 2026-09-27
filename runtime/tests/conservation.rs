//! Requirement "总量守恒" / Scenario "随机交易序列的守恒" (chain/native-token), task 5.4.
//!
//! Random sequences of transfers (with and without tips), first-key registrations, rotations,
//! failing transactions, double-signing reports (m2-finality 6.3), randomness inherents,
//! treasury spends and blocks crossing emission-epoch boundaries (m3-economics 7.2), staking,
//! nominating, unbonding, withdrawing, the switch to PoS with reward payouts and slashing
//! (m3-pos 8.3): after every
//! step the sum of all balances equals the total issuance, and the issuance changed by exactly
//! what emission minted minus what was burned.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_arguments
)]

mod common;

use ac_crypto::SigAlg;
use ac_runtime::transaction::TxParams;
use ac_runtime::{ATC, Emission, RuntimeCall, RuntimeOrigin, System, TreasuryDual};
use common::{
    GENESIS, Signer, apply, build, context, free, issuance, next_authored_block, preset_ext,
    signed, start_authored_block, sum_of_balances, transfer,
};
use proptest::prelude::{ProptestConfig, Strategy, any, prop_oneof, proptest};
use sp_runtime::generic::Era;
use sp_runtime::traits::Dispatchable;

#[derive(Clone, Debug)]
enum Op {
    /// Transfer from signer `from` to signer `to`; `huge` makes it exceed the balance.
    Transfer {
        from: usize,
        to: usize,
        amount: u64,
        huge: bool,
    },
    /// Rotate the key of signer `who` to a fresh ML-DSA-65 key.
    Rotate { who: usize, seed: u8 },
    /// Report the authority's double signing in `slot`; `forged` breaks one seal.
    Report { slot: u64, forged: bool },
    /// Report the authority's vote double signing in `round`; `forged` breaks one signature.
    ReportVote { round: u64, forged: bool },
    /// Apply a randomness inherent with arbitrary data.
    Randomness { commit: u8, reveal: u8 },
    /// Transfer with a tip of `tip` thousandths of an ATC.
    Tipped { from: usize, to: usize, tip: u64 },
    /// Spend from community grants (possibly more than it holds) as the administration.
    Spend { to: usize, amount: u64 },
    /// Produce blocks up to and including the next emission settlement.
    CrossEpoch,
    /// Produce `n` blocks.
    Blocks { n: u8 },
    /// Alice registers her authority key as a candidate with `thousands` × 1,000 ATC.
    Stake { thousands: u16 },
    /// Signer `who` nominates alice with `amount` ATC.
    Nominate { who: usize, amount: u64 },
    /// Signer `who` unbonds `amount` ATC.
    Unbond { who: usize, amount: u64 },
    /// Signer `who` withdraws unlocked stake.
    Withdraw { who: usize },
    /// Produce blocks until the chain is in PoS (at most 60): settlements then pay rewards.
    ToPos,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..6, 0usize..6, 1u64..1_000, any::<bool>()).prop_map(|(from, to, amount, huge)| {
            Op::Transfer {
                from,
                to,
                amount,
                huge,
            }
        }),
        (0usize..6, 100u8..200).prop_map(|(who, seed)| Op::Rotate { who, seed }),
        (0u64..10, any::<bool>()).prop_map(|(slot, forged)| Op::Report { slot, forged }),
        (0u64..10, any::<bool>()).prop_map(|(round, forged)| Op::ReportVote { round, forged }),
        (any::<u8>(), any::<u8>()).prop_map(|(commit, reveal)| Op::Randomness { commit, reveal }),
        (0usize..6, 0usize..6, 0u64..5_000).prop_map(|(from, to, tip)| Op::Tipped {
            from,
            to,
            tip
        }),
        (0usize..6, 1u64..3_000).prop_map(|(to, amount)| Op::Spend { to, amount }),
        proptest::strategy::Just(Op::CrossEpoch),
        (1u8..4).prop_map(|n| Op::Blocks { n }),
        (100u16..900).prop_map(|thousands| Op::Stake { thousands }),
        (1usize..4, 60u64..5_000).prop_map(|(who, amount)| Op::Nominate { who, amount }),
        (0usize..4, 1u64..5_000).prop_map(|(who, amount)| Op::Unbond { who, amount }),
        (0usize..4).prop_map(|who| Op::Withdraw { who }),
        proptest::strategy::Just(Op::ToPos),
    ]
}

proptest! {
    // Default of 256 cases (task 5.4 acceptance); each case runs at most 6 operations.
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn issuance_equals_sum_of_balances(ops in proptest::collection::vec(op(), 1..6)) {
        let mut ext = preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET);
        ext.execute_with(|| start_authored_block(1, GENESIS));
        ext.execute_with(|| {
            // Four endowed development accounts and two fresh ones that must register first.
            let mut signers: Vec<Signer> = ["alice", "bob", "charlie", "dave"]
                .iter()
                .map(|n| Signer::dev(n))
                .collect();
            signers.push(Signer::fresh(1, SigAlg::MlDsa44));
            signers.push(Signer::fresh(2, SigAlg::MlDsa65));
            // Community grants hold 2 ATC so that some spends succeed.
            let community = TreasuryDual::community_account();
            assert_eq!(apply(signed(&signers[0], transfer(&community, 2 * ATC))), Ok(Ok(())));
            assert_eq!(sum_of_balances(), issuance());
            let ledger = || (
                issuance(),
                pallet_emission::TotalMinted::<ac_runtime::Runtime>::get(),
                Emission::total_burned(),
            );
            let mut last = ledger();

            for op in ops {
                match op {
                    Op::Transfer { from, to, amount, huge } => {
                        let value = if huge { free(&signers[from].account) + 1 } else { u128::from(amount) * ATC / 1_000 };
                        let call = transfer(&signers[to].account.clone(), value);
                        let _ = apply(signed(&signers[from], call));
                    }
                    Op::Rotate { who, seed } => {
                        let new = Signer::fresh(seed, SigAlg::MlDsa65);
                        let genesis = frame_system::Pallet::<ac_runtime::Runtime>::block_hash(0);
                        let rotations = ac_runtime::PqAccounts::current_key(&signers[who].account).map_or(0, |(_, r)| r);
                        let statement = pallet_pq_accounts::rotation_statement(&genesis, &signers[who].account, rotations, &new.public());
                        let proof = new.key.sign_deterministic(&statement, pallet_pq_accounts::KEY_ROTATION_CONTEXT).unwrap();
                        let call = RuntimeCall::PqAccounts(pallet_pq_accounts::Call::rotate_key { new_key: new.public(), proof });
                        if apply(signed(&signers[who], call)) == Ok(Ok(())) {
                            signers[who].key = new.key;
                        }
                    }
                    Op::Report { slot, forged } => {
                        let _ = apply(common::report(common::seal_evidence("alice", slot, forged)));
                    }
                    Op::ReportVote { round, forged } => {
                        let _ = apply(common::report(common::vote_evidence("alice", 0, round, forged)));
                    }
                    Op::Randomness { commit, reveal } => {
                        let call = RuntimeCall::RandomnessCr(pallet_randomness_cr::Call::note_randomness {
                            commit: Some([commit; 32]),
                            reveal: Some([reveal; 32]),
                        });
                        let _ = apply(ac_runtime::UncheckedExtrinsic::new_bare(call));
                    }
                    Op::Tipped { from, to, tip } => {
                        let params = TxParams {
                            nonce: System::account_nonce(&signers[from].account),
                            tip: u128::from(tip) * ATC / 1_000,
                            era: Era::Immortal,
                            era_birth_hash: System::block_hash(0),
                        };
                        let first = pallet_pq_accounts::Keys::<ac_runtime::Runtime>::get(&signers[from].account).is_none();
                        let call = transfer(&signers[to].account.clone(), ATC / 100);
                        let xt = build(&signers[from], &signers[from].key, call, first, params, context());
                        let _ = apply(xt);
                    }
                    Op::Spend { to, amount } => {
                        // The dev administration is alice alone, threshold 1.
                        let origin = RuntimeOrigin::from(pallet_collective::RawOrigin::<
                            ac_runtime::AccountId,
                            pallet_collective::Instance1,
                        >::Members(1, 1));
                        let call = RuntimeCall::TreasuryDual(pallet_treasury_dual::Call::spend {
                            to: signers[to].account.clone(),
                            amount: u128::from(amount) * ATC / 1_000,
                        });
                        let _ = call.dispatch(origin);
                    }
                    Op::CrossEpoch => {
                        let length = pallet_emission::EpochLength::<ac_runtime::Runtime>::get().unwrap();
                        loop {
                            next_authored_block();
                            if (u64::from(System::block_number()) - 1) % length == 0 {
                                break;
                            }
                        }
                    }
                    Op::Blocks { n } => {
                        for _ in 0..n {
                            next_authored_block();
                        }
                    }
                    Op::Stake { thousands } => {
                        let authority = ac_crypto::sig::SigningKey::from_seed(
                            SigAlg::MlDsa65,
                            &ac_crypto::dev_seed("alice").unwrap(),
                        )
                        .unwrap();
                        let key = authority.public_key().unwrap();
                        let genesis = frame_system::Pallet::<ac_runtime::Runtime>::block_hash(0);
                        let statement = ac_primitives::staking::pop_statement(&genesis, &signers[0].account, &key);
                        let proof = authority
                            .sign_deterministic(&statement, ac_primitives::staking::VALIDATOR_POP_CONTEXT)
                            .unwrap();
                        let call = RuntimeCall::StakingPos(pallet_staking_pos::Call::register_candidate {
                            key,
                            proof,
                            value: u128::from(thousands) * 1_000 * ATC,
                            commission_bps: 1_000,
                        });
                        let _ = apply(signed(&signers[0], call));
                    }
                    Op::Nominate { who, amount } => {
                        let call = RuntimeCall::StakingPos(pallet_staking_pos::Call::nominate {
                            value: u128::from(amount) * ATC,
                            targets: vec![signers[0].account.clone()],
                        });
                        let _ = apply(signed(&signers[who], call));
                    }
                    Op::Unbond { who, amount } => {
                        let call = RuntimeCall::StakingPos(pallet_staking_pos::Call::unbond {
                            value: u128::from(amount) * ATC,
                        });
                        let _ = apply(signed(&signers[who], call));
                    }
                    Op::Withdraw { who } => {
                        let call = RuntimeCall::StakingPos(pallet_staking_pos::Call::withdraw_unbonded {});
                        let _ = apply(signed(&signers[who], call));
                    }
                    Op::ToPos => {
                        for _ in 0..60 {
                            if pallet_validator_set::Phase::<ac_runtime::Runtime>::get()
                                == ac_primitives::staking::ChainPhase::Pos
                            {
                                break;
                            }
                            next_authored_block();
                        }
                    }
                }
                let now = ledger();
                assert_eq!(sum_of_balances(), now.0);
                // Δissuance = Δminted − Δburned; the counters never decrease.
                assert!(now.1 >= last.1 && now.2 >= last.2);
                assert_eq!(now.0 + (now.2 - last.2), last.0 + (now.1 - last.1));
                last = now;
            }
        });
    }
}
