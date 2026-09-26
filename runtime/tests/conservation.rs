//! Requirement "总量守恒" / Scenario "随机交易序列的守恒" (chain/native-token), task 5.4.
//!
//! Random sequences of transfers, first-key registrations, rotations, failing transactions,
//! double-signing reports (m2-finality 6.3) and randomness inherents: after every step the sum
//! of all balances equals the total issuance, and issuance never grows.
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
use ac_runtime::{ATC, RuntimeCall};
use common::{Signer, apply, dev_ext, free, issuance, signed, sum_of_balances, transfer};
use proptest::prelude::{ProptestConfig, Strategy, any, prop_oneof, proptest};

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
    /// Apply a randomness inherent with arbitrary data.
    Randomness { commit: u8, reveal: u8 },
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
        (any::<u8>(), any::<u8>()).prop_map(|(commit, reveal)| Op::Randomness { commit, reveal }),
    ]
}

proptest! {
    // Default of 256 cases (task 5.4 acceptance); each case runs at most 6 operations.
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn issuance_equals_sum_of_balances(ops in proptest::collection::vec(op(), 1..6)) {
        dev_ext().execute_with(|| {
            // Four endowed development accounts and two fresh ones that must register first.
            let mut signers: Vec<Signer> = ["alice", "bob", "charlie", "dave"]
                .iter()
                .map(|n| Signer::dev(n))
                .collect();
            signers.push(Signer::fresh(1, SigAlg::MlDsa44));
            signers.push(Signer::fresh(2, SigAlg::MlDsa65));
            assert_eq!(sum_of_balances(), issuance());
            let mut last = issuance();

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
                    Op::Randomness { commit, reveal } => {
                        let call = RuntimeCall::RandomnessCr(pallet_randomness_cr::Call::note_randomness {
                            commit: Some([commit; 32]),
                            reveal: Some([reveal; 32]),
                        });
                        let _ = apply(ac_runtime::UncheckedExtrinsic::new_bare(call));
                    }
                }
                let now = issuance();
                assert_eq!(sum_of_balances(), now);
                assert!(now <= last, "issuance grew from {last} to {now}");
                last = now;
            }
        });
    }
}
