//! Unit tests of the AC-BFT state machine. Test names and comments cite the Requirement /
//! Scenario of `consensus/ac-bft` they cover.

use super::*;
use crate::sim::{TreeChain, dummy_authorities, dummy_signature};

const SLOT: u64 = 1000;

fn config(n: usize, me: Option<u16>, set_id: SetId) -> Config {
    Config {
        set_id,
        authorities: dummy_authorities(n),
        me,
        slot_ms: SLOT,
    }
}

fn signed(signer: u16, message: Message) -> SignedMessage {
    SignedMessage {
        set_id: 0,
        signer,
        message,
        signature: dummy_signature(),
    }
}

fn vote(kind: VoteKind, round: Round, target: BlockRef) -> Message {
    Message::Vote {
        kind,
        round,
        target,
    }
}

fn proposal(round: Round, target: BlockRef, justify: Option<CertRef>) -> Message {
    Message::Proposal {
        round,
        target,
        justify,
    }
}

fn broadcasts(actions: &[Action]) -> Vec<Message> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Broadcast(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn finalized(actions: &[Action]) -> Vec<FinalityProof> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Finalize(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// Feeds `message` from each of `signers`, collecting all actions.
fn feed(
    voter: &mut Voter,
    chain: &TreeChain,
    signers: &[u16],
    message: &Message,
    now: u64,
) -> Vec<Action> {
    let mut all = Vec::new();
    for s in signers {
        all.extend(voter.on_message(signed(*s, message.clone()), chain, now));
    }
    all
}

/// Feeds back this voter's own broadcasts (as the node does after signing).
fn loopback(voter: &mut Voter, chain: &TreeChain, actions: &[Action], now: u64) -> Vec<Action> {
    let me = voter.config().me.unwrap();
    let mut all = Vec::new();
    for m in broadcasts(actions) {
        let more = voter.on_message(signed(me, m), chain, now);
        all.extend(more.clone());
        all.extend(loopback(voter, chain, &more, now));
    }
    all
}

// Requirement "两阶段投票与阈值": leaders rotate r mod n.
#[test]
fn leaders_rotate() {
    let voter = Voter::new(config(4, Some(1), 0), None, TreeChain::genesis());
    let leaders: Vec<_> = (0..9).map(|r| voter.leader(r).unwrap()).collect();
    assert_eq!(leaders, vec![0, 1, 2, 3, 0, 1, 2, 3, 0]);
}

// Requirement "授权节点集合切换": a new set starts at round 0 and ignores the old set's state.
#[test]
fn new_set_restarts_rounds() {
    let old = VoterState {
        set_id: 0,
        round: 17,
        lock: None,
        signed: Vec::new(),
    };
    let voter = Voter::new(
        config(3, Some(0), 1),
        Some(old.clone()),
        TreeChain::genesis(),
    );
    assert_eq!(voter.round(), 0);
    let same = Voter::new(config(3, Some(0), 0), Some(old), TreeChain::genesis());
    assert_eq!(same.round(), 17);
}

// Requirement "两阶段投票与阈值": prepare on a valid proposal, commit on a prepare certificate,
// finalize on a commit certificate; the proof lists the commit signers in order.
#[test]
fn two_phases_finalize() {
    let mut chain = TreeChain::new();
    let b1 = chain.child(&TreeChain::genesis(), 1);
    let mut voter = Voter::new(config(4, Some(1), 0), None, TreeChain::genesis());

    let actions = voter.on_message(signed(0, proposal(0, b1, None)), &chain, 10);
    assert_eq!(broadcasts(&actions), vec![vote(VoteKind::Prepare, 0, b1)]);
    let mut actions = loopback(&mut voter, &chain, &actions, 10);
    actions.extend(feed(
        &mut voter,
        &chain,
        &[0, 2],
        &vote(VoteKind::Prepare, 0, b1),
        20,
    ));
    assert_eq!(broadcasts(&actions), vec![vote(VoteKind::Commit, 0, b1)]);
    assert_eq!(
        voter.lock(),
        Some(CertRef {
            round: 0,
            target: b1
        })
    );
    assert_eq!(
        voter.round(),
        1,
        "pipelined: next round starts with the commit phase"
    );

    let mut actions = loopback(&mut voter, &chain, &actions, 20);
    assert!(finalized(&actions).is_empty());
    actions.extend(feed(
        &mut voter,
        &chain,
        &[3],
        &vote(VoteKind::Commit, 0, b1),
        30,
    ));
    assert!(
        finalized(&actions).is_empty(),
        "two commits of four are not a certificate"
    );
    actions.extend(feed(
        &mut voter,
        &chain,
        &[0],
        &vote(VoteKind::Commit, 0, b1),
        31,
    ));
    let proofs = finalized(&actions);
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].target, b1);
    let signers: Vec<_> = proofs[0].commits.iter().map(|(i, _)| *i).collect();
    assert_eq!(signers, vec![0, 1, 3]);
    assert_eq!(voter.finalized(), b1);
}

// Scenario "票数不足不最终确定": two of four commit votes finalize nothing.
#[test]
fn two_commits_of_four_are_not_enough() {
    let mut chain = TreeChain::new();
    let b1 = chain.child(&TreeChain::genesis(), 1);
    let mut observer = Voter::new(config(4, None, 0), None, TreeChain::genesis());
    let actions = feed(
        &mut observer,
        &chain,
        &[0, 2],
        &vote(VoteKind::Commit, 0, b1),
        5,
    );
    assert!(finalized(&actions).is_empty());
    assert!(broadcasts(&actions).is_empty(), "observers never sign");
    let actions = feed(
        &mut observer,
        &chain,
        &[3],
        &vote(VoteKind::Commit, 0, b1),
        6,
    );
    assert_eq!(finalized(&actions).len(), 1);
}

// Scenario "单节点开发链" (protocol part): one member finalizes on its own.
#[test]
fn single_member_finalizes_alone() {
    let mut chain = TreeChain::new();
    let b1 = chain.child(&TreeChain::genesis(), 1);
    let mut voter = Voter::new(config(1, Some(0), 0), None, TreeChain::genesis());
    let actions = voter.on_block_imported(&chain, 0);
    let all = loopback(&mut voter, &chain, &actions, 0);
    assert_eq!(finalized(&all).first().map(|p| p.target), Some(b1));
}

// Requirement "两阶段投票与阈值": the target must extend the last finalized block.
#[test]
fn proposals_must_extend_finalized() {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let a1 = chain.child(&g, 1);
    let b1 = chain.child(&g, 2);
    let mut voter = Voter::new(config(4, Some(1), 0), None, a1);
    let actions = voter.on_message(signed(0, proposal(0, b1, None)), &chain, 1);
    assert!(broadcasts(&actions).is_empty());
    let a2 = chain.child(&a1, 3);
    let mut voter = Voter::new(config(4, Some(1), 0), None, a1);
    let actions = voter.on_message(signed(0, proposal(0, a2, None)), &chain, 1);
    assert_eq!(broadcasts(&actions), vec![vote(VoteKind::Prepare, 0, a2)]);
}

/// A member locked on `(0, b1)` that moved to round 2 through timeouts of round 1, having
/// seen a prepare certificate `(1, c1)` for a conflicting block.
fn locked_voter() -> (Voter, TreeChain, BlockRef, BlockRef) {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let b1 = chain.child(&g, 1);
    let c1 = chain.child(&g, 2);
    let c2 = chain.child(&c1, 3);
    let mut voter = Voter::new(config(4, Some(1), 0), None, g);
    let a = voter.on_message(signed(0, proposal(0, b1, None)), &chain, 1);
    let mut a2 = loopback(&mut voter, &chain, &a, 1);
    a2.extend(feed(
        &mut voter,
        &chain,
        &[0, 2],
        &vote(VoteKind::Prepare, 0, b1),
        2,
    ));
    loopback(&mut voter, &chain, &a2, 2);
    assert_eq!(
        voter.lock(),
        Some(CertRef {
            round: 0,
            target: b1
        })
    );
    feed(
        &mut voter,
        &chain,
        &[0, 2, 3],
        &Message::Timeout {
            round: 1,
            high: None,
        },
        3,
    );
    assert_eq!(voter.round(), 2);
    feed(
        &mut voter,
        &chain,
        &[0, 2, 3],
        &vote(VoteKind::Prepare, 1, c1),
        4,
    );
    assert_eq!(
        voter.high(),
        Some(CertRef {
            round: 1,
            target: c1
        })
    );
    assert_eq!(
        voter.lock(),
        Some(CertRef {
            round: 0,
            target: b1
        }),
        "no commit for a past round"
    );
    (voter, chain, c1, c2)
}

// Requirement "安全性": a locked member refuses a conflicting proposal without a higher
// certificate…
#[test]
fn lock_blocks_conflicting_proposal() {
    let (mut voter, chain, _c1, c2) = locked_voter();
    let actions = voter.on_message(signed(2, proposal(2, c2, None)), &chain, 5);
    assert!(broadcasts(&actions).is_empty());
}

// …but accepts one justified by a certificate of a round above its lock that it has seen.
#[test]
fn higher_certificate_unlocks() {
    let (mut voter, chain, c1, c2) = locked_voter();
    let justify = Some(CertRef {
        round: 1,
        target: c1,
    });
    let actions = voter.on_message(signed(2, proposal(2, c2, justify)), &chain, 5);
    assert_eq!(broadcasts(&actions), vec![vote(VoteKind::Prepare, 2, c2)]);
}

// Justifications must be certificates this node has seen, from an earlier round.
#[test]
fn unseen_or_same_round_justification_is_refused() {
    let (mut voter, chain, c1, c2) = locked_voter();
    let unseen = Some(CertRef {
        round: 1,
        target: c2,
    });
    assert!(
        broadcasts(&voter.on_message(signed(2, proposal(2, c2, unseen)), &chain, 5)).is_empty()
    );
    let (mut voter, chain, _, c2) = locked_voter();
    let same_round = Some(CertRef {
        round: 2,
        target: c1,
    });
    assert!(
        broadcasts(&voter.on_message(signed(2, proposal(2, c2, same_round)), &chain, 5)).is_empty()
    );
}

// Scenario "领导者离线": the round times out, q timeouts start the next round, whose leader
// waits half a slot and proposes.
#[test]
fn offline_leader_times_out() {
    let mut chain = TreeChain::new();
    let b1 = chain.child(&TreeChain::genesis(), 1);
    let mut voter = Voter::new(config(4, Some(1), 0), None, TreeChain::genesis());
    assert!(voter.on_block_imported(&chain, 0).is_empty());
    assert_eq!(voter.next_deadline(), Some(2 * SLOT));
    let actions = voter.on_tick(&chain, 2 * SLOT);
    assert_eq!(
        broadcasts(&actions),
        vec![Message::Timeout {
            round: 0,
            high: None
        }]
    );
    let mut all = loopback(&mut voter, &chain, &actions, 2 * SLOT);
    all.extend(feed(
        &mut voter,
        &chain,
        &[2, 3],
        &Message::Timeout {
            round: 0,
            high: None,
        },
        2100,
    ));
    assert_eq!(voter.round(), 1);
    assert!(
        broadcasts(&all)
            .iter()
            .all(|m| !matches!(m, Message::Proposal { .. }))
    );
    assert_eq!(voter.next_deadline(), Some(2100 + SLOT / 2));
    let actions = voter.on_tick(&chain, 2100 + SLOT / 2);
    assert_eq!(broadcasts(&actions), vec![proposal(1, b1, None)]);
}

// Exponential backoff ×1.5, capped.
#[test]
fn timeout_backoff() {
    let voter = Voter::new(config(4, Some(1), 0), None, TreeChain::genesis());
    assert_eq!(voter.timeout_ms(0), 2000);
    assert_eq!(voter.timeout_ms(1), 3000);
    assert_eq!(voter.timeout_ms(2), 4500);
    assert_eq!(voter.timeout_ms(50), MAX_TIMEOUT_MS);
}

// Catch-up: messages of a higher round from more than W − q weight move a member forward;
// from less they do not.
#[test]
fn catch_up_needs_an_honest_member() {
    let chain = TreeChain::new();
    let mut voter = Voter::new(config(4, Some(1), 0), None, TreeChain::genesis());
    feed(
        &mut voter,
        &chain,
        &[2],
        &Message::Timeout {
            round: 5,
            high: None,
        },
        1,
    );
    assert_eq!(voter.round(), 0);
    feed(
        &mut voter,
        &chain,
        &[3],
        &Message::Timeout {
            round: 6,
            high: None,
        },
        2,
    );
    assert_eq!(voter.round(), 5);
}

// Scenario "新节点通过证明跟上终局性" (protocol part): a node more than `FUTURE_ROUNDS` behind,
// such as a full node joining a running network, catches up from the rounds it sees and then
// follows finality; a single member far ahead does not move it.
#[test]
fn far_behind_node_catches_up() {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let b1 = chain.child(&g, 1);
    let far = FUTURE_ROUNDS * 3;
    let mut follower = Voter::new(config(4, None, 0), None, g);
    let commit = vote(VoteKind::Commit, far, b1);
    let actions = follower.on_message(signed(0, commit.clone()), &chain, 1);
    assert!(actions.is_empty());
    assert_eq!(follower.round(), 0, "one member ahead is not enough");
    for signer in 1..4 {
        follower.on_message(signed(signer, commit.clone()), &chain, 2);
    }
    assert_eq!(follower.round(), far);
    assert_eq!(follower.finalized(), b1);
}

// Scenario "集合变更后继续最终确定": the old set finalizes the change block and never goes past
// it; the new set continues from there.
#[test]
fn set_change_handover() {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let b1 = chain.child(&g, 1);
    chain.mark_change(&b1);
    let b2 = chain.child(&b1, 2);

    // The old set's leader proposes the change block, not the best block.
    let mut leader = Voter::new(config(4, Some(0), 0), None, g);
    let actions = leader.on_block_imported(&chain, 0);
    assert_eq!(broadcasts(&actions), vec![proposal(0, b1, None)]);
    // Members refuse targets past the change block.
    let mut member = Voter::new(config(4, Some(1), 0), None, g);
    assert!(broadcasts(&member.on_message(signed(0, proposal(0, b2, None)), &chain, 1)).is_empty());
    // Commit certificates past the change block finalize nothing.
    let mut observer = Voter::new(config(4, None, 0), None, g);
    assert!(
        finalized(&feed(
            &mut observer,
            &chain,
            &[0, 1, 2],
            &vote(VoteKind::Commit, 0, b2),
            1
        ))
        .is_empty()
    );
    let proofs = finalized(&feed(
        &mut observer,
        &chain,
        &[0, 1, 2],
        &vote(VoteKind::Commit, 1, b1),
        2,
    ));
    assert_eq!((proofs[0].set_id, proofs[0].target), (0, b1));

    // New set (id 1, three members) starting from the change block.
    let mut next = Voter::new(config(3, None, 1), None, b1);
    let msg = |s: u16| SignedMessage {
        set_id: 1,
        signer: s,
        message: vote(VoteKind::Commit, 0, b2),
        signature: dummy_signature(),
    };
    let mut actions = Vec::new();
    for s in 0..3 {
        actions.extend(next.on_message(msg(s), &chain, 4));
    }
    let proofs = finalized(&actions);
    assert_eq!((proofs[0].set_id, proofs[0].target), (1, b2));
    // Messages of the old set are ignored by the new one.
    assert!(
        next.on_message(signed(0, vote(VoteKind::Commit, 1, b2)), &chain, 5)
            .is_empty()
    );
}

// Requirement "重启后不签冲突消息" (protocol part) and the evidence path: conflicting messages
// from one member produce one report; timeouts never do.
#[test]
fn equivocation_is_reported_once() {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let a = chain.child(&g, 1);
    let b = chain.child(&g, 2);
    let mut observer = Voter::new(config(4, None, 0), None, g);
    let reports = |actions: &[Action]| {
        actions
            .iter()
            .filter(|x| matches!(x, Action::Report(_)))
            .count()
    };
    assert_eq!(
        reports(&feed(
            &mut observer,
            &chain,
            &[2],
            &vote(VoteKind::Prepare, 0, a),
            1
        )),
        0
    );
    assert_eq!(
        reports(&feed(
            &mut observer,
            &chain,
            &[2],
            &vote(VoteKind::Prepare, 0, a),
            2
        )),
        0
    );
    assert_eq!(
        reports(&feed(
            &mut observer,
            &chain,
            &[2],
            &vote(VoteKind::Prepare, 0, b),
            3
        )),
        1
    );
    let c = chain.child(&g, 3);
    assert_eq!(
        reports(&feed(
            &mut observer,
            &chain,
            &[2],
            &vote(VoteKind::Prepare, 0, c),
            4
        )),
        0
    );
    let t1 = Message::Timeout {
        round: 0,
        high: None,
    };
    let t2 = Message::Timeout {
        round: 0,
        high: Some(CertRef {
            round: 0,
            target: a,
        }),
    };
    assert_eq!(reports(&feed(&mut observer, &chain, &[3], &t1, 5)), 0);
    assert_eq!(reports(&feed(&mut observer, &chain, &[3], &t2, 6)), 0);
}

// Every broadcast is immediately preceded by the persisted state that records it.
#[test]
fn persist_precedes_broadcast() {
    let mut chain = TreeChain::new();
    let b1 = chain.child(&TreeChain::genesis(), 1);
    let mut voter = Voter::new(config(1, Some(0), 0), None, TreeChain::genesis());
    let first = voter.on_block_imported(&chain, 0);
    let all: Vec<Action> = first
        .iter()
        .cloned()
        .chain(loopback(&mut voter, &chain, &first, 0))
        .collect();
    let mut previous: Option<&Action> = None;
    let mut count = 0;
    for action in &all {
        if let Action::Broadcast(message) = action {
            count += 1;
            match previous {
                Some(Action::Persist(state)) => {
                    let (kind, round) = (message.kind(), message.round());
                    assert!(state.signed.contains(&(kind, round, message.clone())));
                }
                other => panic!("broadcast not preceded by persist: {other:?}"),
            }
        }
        previous = Some(action);
    }
    assert_eq!(count, 3, "proposal, prepare and commit");
    assert_eq!(voter.finalized(), b1);
}

// Scenario "重启后不双签" (protocol part): after a restart from the persisted state, a member
// does not prepare a different target in the same round.
#[test]
fn restart_never_signs_conflicting_message() {
    let mut chain = TreeChain::new();
    let g = TreeChain::genesis();
    let a = chain.child(&g, 1);
    let b = chain.child(&g, 2);
    let mut voter = Voter::new(config(4, Some(1), 0), None, g);
    let actions = voter.on_message(signed(0, proposal(0, a, None)), &chain, 1);
    let state = actions
        .iter()
        .find_map(|x| match x {
            Action::Persist(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    // Crash; restart from the persisted state; an equivocating leader proposes b.
    let mut restarted = Voter::new(config(4, Some(1), 0), Some(state), g);
    let actions = restarted.on_message(signed(0, proposal(0, b, None)), &chain, 101);
    assert!(broadcasts(&actions).is_empty());
    // Re-sending the identical vote stays allowed.
    let mut again = Voter::new(config(4, Some(1), 0), Some(restarted_state(&restarted)), g);
    let actions = again.on_message(signed(0, proposal(0, a, None)), &chain, 201);
    assert_eq!(broadcasts(&actions), vec![vote(VoteKind::Prepare, 0, a)]);
}

fn restarted_state(voter: &Voter) -> VoterState {
    voter.state.clone()
}
