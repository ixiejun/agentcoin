//! Deterministic network simulator for the AC-BFT state machine (design D12 of
//! `m2-finality`). Test-only: compiled for this crate's tests and with the `test-utils`
//! feature.
//!
//! Blocks are produced every slot by a round-robin author and reach nodes after random
//! delays; consensus messages are delayed, dropped and reordered at random until the global
//! stabilization time (GST), and delivered within a small bound afterwards. Up to `f` nodes
//! are Byzantine. Signatures are placeholders: the state machine expects verified input, and
//! signature checks are tested separately in `ac-primitives`.
// Test tooling: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::rc::Rc;

use ac_crypto::{PqSignature, SigAlg};
use ac_primitives::ac_bft::{Authority, BlockRef, Message, SignedMessage, VoteKind};
use sp_core::H256;

use crate::protocol::{Action, Chain, Config, Voter};

/// Small deterministic PRNG (SplitMix64); test use only.
#[derive(Clone, Debug)]
pub struct TestRng(u64);

impl TestRng {
    /// Seeded generator.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// `true` with probability `percent / 100`.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// In-memory block tree with a per-node set of known blocks.
#[derive(Clone, Debug, Default)]
pub struct TreeChain {
    parents: BTreeMap<H256, (Option<H256>, u32)>,
    known: BTreeSet<H256>,
    changes: BTreeSet<H256>,
    best: Option<BlockRef>,
    finalized: Option<BlockRef>,
}

impl TreeChain {
    /// A tree holding only the genesis block (hash zero, number 0), known.
    #[must_use]
    pub fn new() -> Self {
        let mut chain = Self::default();
        chain.parents.insert(H256::zero(), (None, 0));
        chain.known.insert(H256::zero());
        chain.best = Some(Self::genesis());
        chain
    }

    /// Records finality: like the node's fork choice, the best block from now on is the
    /// longest chain through `block`.
    pub fn set_finalized(&mut self, block: BlockRef) {
        self.finalized = Some(block);
        self.best = Some(block);
        let known: Vec<H256> = self.known.iter().copied().collect();
        for h in known {
            let b = self.block(h).unwrap();
            self.note_known(b);
        }
    }

    fn note_known(&mut self, block: BlockRef) {
        if let Some(f) = self.finalized
            && !self.is_descendant(&block, &f)
        {
            return;
        }
        let better = self.best.is_none_or(|b| {
            block.number > b.number || (block.number == b.number && block.hash < b.hash)
        });
        if better {
            self.best = Some(block);
        }
    }

    /// The genesis block.
    #[must_use]
    pub fn genesis() -> BlockRef {
        BlockRef {
            hash: H256::zero(),
            number: 0,
        }
    }

    /// Adds a known block with the given hash on top of `parent`.
    pub fn add(&mut self, hash: H256, parent: &BlockRef) -> BlockRef {
        let number = parent.number + 1;
        self.parents.insert(hash, (Some(parent.hash), number));
        self.known.insert(hash);
        let block = BlockRef { hash, number };
        self.note_known(block);
        block
    }

    /// Adds a known child of `parent` whose hash is derived from `tag`.
    pub fn child(&mut self, parent: &BlockRef, tag: u64) -> BlockRef {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&tag.to_le_bytes());
        bytes[8..12].copy_from_slice(&(parent.number + 1).to_le_bytes());
        bytes[31] = 1;
        self.add(H256(bytes), parent)
    }

    /// Marks `block` as announcing an authority-set change.
    pub fn mark_change(&mut self, block: &BlockRef) {
        self.changes.insert(block.hash);
    }

    /// Makes a block (already in the tree) known, with all its ancestors.
    pub fn learn(&mut self, hash: H256, from: &TreeChain) {
        let mut path = Vec::new();
        let mut cursor = Some(hash);
        while let Some(h) = cursor {
            if self.known.contains(&h) {
                break;
            }
            let entry = from.parents[&h];
            path.push((h, entry));
            cursor = entry.0;
        }
        for (h, entry) in path.into_iter().rev() {
            self.known.insert(h);
            self.parents.insert(h, entry);
            if from.changes.contains(&h) {
                self.changes.insert(h);
            }
            self.note_known(BlockRef {
                hash: h,
                number: entry.1,
            });
        }
    }

    fn block(&self, hash: H256) -> Option<BlockRef> {
        self.known.contains(&hash).then(|| BlockRef {
            hash,
            number: self.parents[&hash].1,
        })
    }

    fn parent(&self, block: &BlockRef) -> Option<BlockRef> {
        let parent = self.parents.get(&block.hash)?.0?;
        self.block(parent)
    }
}

impl Chain for TreeChain {
    fn contains(&self, block: &BlockRef) -> bool {
        self.block(block.hash) == Some(*block)
    }

    fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool {
        if !self.contains(block) || !self.contains(ancestor) {
            return false;
        }
        let mut cursor = *block;
        while cursor.number > ancestor.number {
            match self.parent(&cursor) {
                Some(p) => cursor = p,
                None => return false,
            }
        }
        cursor == *ancestor
    }

    fn best(&self) -> BlockRef {
        self.best.unwrap_or_else(Self::genesis)
    }

    fn first_change(&self, after: &BlockRef, upto: &BlockRef) -> Option<BlockRef> {
        let mut found = None;
        let mut cursor = *upto;
        while cursor.number > after.number {
            if self.changes.contains(&cursor.hash) {
                found = Some(cursor);
            }
            cursor = self.parent(&cursor)?;
        }
        found
    }
}

/// `n` members with distinct placeholder ML-DSA-65 keys and weight 1.
#[must_use]
pub fn dummy_authorities(n: usize) -> Vec<Authority> {
    (0..n)
        .map(|i| {
            let mut key = vec![0u8; SigAlg::MlDsa65.public_key_len().unwrap()];
            key[..8].copy_from_slice(&u64::try_from(i).unwrap().to_le_bytes());
            Authority::poa(ac_crypto::PqPublicKey::new(SigAlg::MlDsa65, &key).unwrap())
        })
        .collect()
}

/// A placeholder ML-DSA-65 signature (all zeros); the state machine never checks signatures.
#[must_use]
pub fn dummy_signature() -> PqSignature {
    PqSignature::new(
        SigAlg::MlDsa65,
        &vec![0u8; SigAlg::MlDsa65.signature_len().unwrap()],
    )
    .unwrap()
}

/// How a Byzantine node behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Byzantine {
    /// Sends nothing.
    Silent,
    /// Votes prepare and commit for every proposal it sees and for a random other block, and
    /// as leader sends conflicting proposals to different peers.
    Equivocate,
}

/// Simulation parameters.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Number of members.
    pub n: usize,
    /// Byzantine members (indices) and their behaviour.
    pub byzantine: Vec<(usize, Byzantine)>,
    /// Slot duration.
    pub slot_ms: u64,
    /// Global stabilization time: before it, messages are delayed up to `chaos_delay_ms` and
    /// dropped with `drop_percent`; after it, delivered within `calm_delay_ms`.
    pub gst_ms: u64,
    /// Maximum delay before GST.
    pub chaos_delay_ms: u64,
    /// Drop probability before GST, in percent.
    pub drop_percent: u64,
    /// Maximum delay after GST.
    pub calm_delay_ms: u64,
    /// Total simulated time.
    pub duration_ms: u64,
}

impl Scenario {
    /// A random scenario for `n` members with up to `f` Byzantine ones.
    #[must_use]
    pub fn random(n: usize, rng: &mut TestRng) -> Self {
        let f = (n - 1) / 3;
        let faulty = usize::try_from(rng.below(u64::try_from(f).unwrap() + 1)).unwrap();
        let mut byzantine = Vec::new();
        while byzantine.len() < faulty {
            let i = usize::try_from(rng.below(u64::try_from(n).unwrap())).unwrap();
            if byzantine.iter().all(|(j, _)| *j != i) {
                let kind = if rng.chance(50) {
                    Byzantine::Silent
                } else {
                    Byzantine::Equivocate
                };
                byzantine.push((i, kind));
            }
        }
        Self {
            n,
            byzantine,
            slot_ms: 1000,
            gst_ms: 5_000 + rng.below(10_000),
            chaos_delay_ms: 200 + rng.below(3_000),
            drop_percent: rng.below(40),
            calm_delay_ms: 50 + rng.below(150),
            duration_ms: 45_000,
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    Produce,
    Block { to: usize, hash: H256 },
    Deliver { to: usize, msg: Rc<SignedMessage> },
    Tick { to: usize },
}

/// Outcome of a simulation run.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Finalized blocks per honest node, in order.
    pub finalized: BTreeMap<usize, Vec<BlockRef>>,
    /// Highest finalized number per honest node at GST.
    pub at_gst: BTreeMap<usize, u32>,
    /// Conflicting commit certificates observed by honest voters.
    pub conflicts: u64,
    /// Number of evidence reports produced by honest nodes.
    pub reports: usize,
    /// Every block produced (the global tree).
    pub tree: TreeChain,
    /// Final state of each honest voter, for failure messages.
    pub states: Vec<String>,
}

impl Outcome {
    /// Checks that all blocks finalized by honest nodes lie on one chain.
    ///
    /// # Panics
    ///
    /// If two finalized blocks conflict (test helper).
    pub fn assert_safe(&self) {
        assert_eq!(self.conflicts, 0, "conflicting commit certificates");
        let all: BTreeSet<BlockRef> = self.finalized.values().flatten().copied().collect();
        let all: Vec<_> = all.into_iter().collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                let ok = self.tree.is_descendant(a, b) || self.tree.is_descendant(b, a);
                assert!(ok, "conflicting finalized blocks {a:?} and {b:?}");
            }
        }
    }

    /// Whether every honest node finalized a block above what it had at GST.
    #[must_use]
    pub fn all_progressed_after_gst(&self) -> bool {
        self.finalized.iter().all(|(i, blocks)| {
            let before = self.at_gst.get(i).copied().unwrap_or(0);
            blocks.last().is_some_and(|b| b.number > before)
        })
    }
}

/// Runs one scenario with the given seed.
#[must_use]
pub fn run(scenario: &Scenario, seed: u64) -> Outcome {
    Sim::new(scenario.clone(), seed).run()
}

struct Sim {
    scenario: Scenario,
    rng: TestRng,
    now: u64,
    seq: u64,
    queue: BinaryHeap<Reverse<(u64, u64)>>,
    events: BTreeMap<u64, Event>,
    tree: TreeChain,
    views: Vec<TreeChain>,
    voters: Vec<Option<Voter>>,
    byzantine: BTreeMap<usize, Byzantine>,
    signature: PqSignature,
    scheduled: Vec<Option<u64>>,
    outcome: Outcome,
    produced: u64,
    byz_rounds: BTreeMap<usize, u64>,
    byz_seen: BTreeSet<(usize, u64, BlockRef)>,
}

impl Sim {
    fn new(scenario: Scenario, seed: u64) -> Self {
        let authorities = dummy_authorities(scenario.n);
        let byzantine: BTreeMap<_, _> = scenario.byzantine.iter().copied().collect();
        let voters = (0..scenario.n)
            .map(|i| {
                (!byzantine.contains_key(&i)).then(|| {
                    Voter::new(
                        Config {
                            set_id: 0,
                            authorities: authorities.clone(),
                            me: Some(u16::try_from(i).unwrap()),
                            slot_ms: scenario.slot_ms,
                        },
                        None,
                        TreeChain::genesis(),
                    )
                })
            })
            .collect();
        let signature = dummy_signature();
        let n = scenario.n;
        let mut sim = Self {
            scenario,
            rng: TestRng::new(seed),
            now: 0,
            seq: 0,
            queue: BinaryHeap::new(),
            events: BTreeMap::new(),
            tree: TreeChain::new(),
            views: vec![TreeChain::new(); n],
            voters,
            byzantine,
            signature,
            scheduled: vec![None; n],
            outcome: Outcome::default(),
            produced: 0,
            byz_rounds: BTreeMap::new(),
            byz_seen: BTreeSet::new(),
        };
        sim.push(sim.scenario.slot_ms, Event::Produce);
        sim
    }

    fn push(&mut self, at: u64, event: Event) {
        self.seq += 1;
        self.events.insert(self.seq, event);
        self.queue.push(Reverse((at, self.seq)));
    }

    fn delay(&mut self) -> Option<u64> {
        if self.now < self.scenario.gst_ms {
            if self.rng.chance(self.scenario.drop_percent) {
                return None;
            }
            Some(self.rng.below(self.scenario.chaos_delay_ms + 1))
        } else {
            Some(self.rng.below(self.scenario.calm_delay_ms + 1))
        }
    }

    fn run(mut self) -> Outcome {
        for i in 0..self.scenario.n {
            if self.voters[i].is_some() {
                self.outcome.finalized.insert(i, Vec::new());
            }
        }
        let mut gst_recorded = false;
        while let Some(Reverse((at, seq))) = self.queue.pop() {
            let event = self.events.remove(&seq).unwrap();
            if at > self.scenario.duration_ms {
                break;
            }
            if !gst_recorded && at >= self.scenario.gst_ms {
                gst_recorded = true;
                for (i, blocks) in &self.outcome.finalized {
                    let n = blocks.last().map_or(0, |b| b.number);
                    self.outcome.at_gst.insert(*i, n);
                }
            }
            self.now = at;
            match event {
                Event::Produce => self.produce(),
                Event::Block { to, hash } => {
                    self.views[to].learn(hash, &self.tree);
                    if let Some(voter) = self.voters[to].as_mut() {
                        let actions = voter.on_block_imported(&self.views[to], self.now);
                        self.apply(to, actions);
                    }
                }
                Event::Deliver { to, msg } => self.deliver(to, &msg),
                Event::Tick { to } => {
                    self.scheduled[to] = None;
                    if let Some(voter) = self.voters[to].as_mut() {
                        let actions = voter.on_tick(&self.views[to], self.now);
                        self.apply(to, actions);
                    }
                }
            }
        }
        for (i, voter) in self.voters.iter().enumerate() {
            if let Some(voter) = voter {
                self.outcome.conflicts += voter.conflicts();
                self.outcome.states.push(format!(
                    "node {i}: round {} lock {:?} high {:?} finalized {} deadline {:?} best {} {}",
                    voter.round(),
                    voter.lock().map(|c| (c.round, c.target.number)),
                    voter.high().map(|c| (c.round, c.target.number)),
                    voter.finalized().number,
                    voter.next_deadline(),
                    self.views[i].best().number,
                    voter.debug_round(),
                ));
            }
        }
        self.outcome.tree = self.tree;
        self.outcome
    }

    fn produce(&mut self) {
        self.produced += 1;
        let n = u64::try_from(self.scenario.n).unwrap();
        let author = usize::try_from(self.produced % n).unwrap();
        let silent = self.byzantine.get(&author) == Some(&Byzantine::Silent);
        if !silent {
            let mut parent = self.views[author].best();
            // Byzantine authors sometimes build on an older block to create forks.
            if self.byzantine.contains_key(&author) && self.rng.chance(50) {
                let chain = &self.views[author];
                for _ in 0..self.rng.below(3) {
                    if let Some(p) = chain.parent(&parent) {
                        parent = p;
                    }
                }
            }
            let block = self.tree.child(&parent, self.produced);
            self.views[author].learn(block.hash, &self.tree);
            for to in 0..self.scenario.n {
                if to == author {
                    self.push(
                        self.now,
                        Event::Block {
                            to,
                            hash: block.hash,
                        },
                    );
                } else {
                    // Blocks are gossiped and synced, so they eventually arrive even before
                    // GST; only their delay is random.
                    let d = self.rng.below(
                        self.scenario
                            .chaos_delay_ms
                            .max(self.scenario.calm_delay_ms)
                            + 1,
                    );
                    let d = if self.now >= self.scenario.gst_ms {
                        d.min(self.scenario.calm_delay_ms)
                    } else {
                        d
                    };
                    self.push(
                        self.now + d,
                        Event::Block {
                            to,
                            hash: block.hash,
                        },
                    );
                }
            }
        }
        self.push(self.now + self.scenario.slot_ms, Event::Produce);
    }

    fn deliver(&mut self, to: usize, msg: &SignedMessage) {
        if let Some(voter) = self.voters[to].as_mut() {
            let actions = voter.on_message(msg.clone(), &self.views[to], self.now);
            self.apply(to, actions);
        } else if self.byzantine.get(&to) == Some(&Byzantine::Equivocate)
            && !self.byzantine.contains_key(&usize::from(msg.signer))
        {
            self.misbehave(to, msg);
        }
    }

    fn apply(&mut self, from: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Persist(_) => {}
                Action::Broadcast(message) => {
                    let signed = Rc::new(self.signed(from, message));
                    self.push(
                        self.now,
                        Event::Deliver {
                            to: from,
                            msg: Rc::clone(&signed),
                        },
                    );
                    self.broadcast(from, &signed, None);
                }
                Action::Finalize(proof) => {
                    self.outcome
                        .finalized
                        .get_mut(&from)
                        .unwrap()
                        .push(proof.target);
                    self.views[from].set_finalized(proof.target);
                }
                Action::Report(_) => self.outcome.reports += 1,
                Action::Relay(messages) => {
                    for m in messages {
                        self.broadcast(from, &Rc::new(m), None);
                    }
                }
            }
        }
        self.schedule(from);
    }

    fn schedule(&mut self, node: usize) {
        let Some(deadline) = self.voters[node].as_ref().and_then(Voter::next_deadline) else {
            return;
        };
        let deadline = deadline.max(self.now);
        if self.scheduled[node].is_none_or(|s| deadline < s) {
            self.scheduled[node] = Some(deadline);
            self.push(deadline, Event::Tick { to: node });
        }
    }

    fn signed(&self, from: usize, message: Message) -> SignedMessage {
        SignedMessage {
            set_id: 0,
            signer: u16::try_from(from).unwrap(),
            message,
            signature: self.signature.clone(),
        }
    }

    /// Sends to every other node, or to those `filter` accepts.
    fn broadcast(
        &mut self,
        from: usize,
        signed: &Rc<SignedMessage>,
        filter: Option<&dyn Fn(usize) -> bool>,
    ) {
        for to in 0..self.scenario.n {
            if to == from || filter.is_some_and(|f| !f(to)) {
                continue;
            }
            if let Some(d) = self.delay() {
                self.push(
                    self.now + d,
                    Event::Deliver {
                        to,
                        msg: Rc::clone(signed),
                    },
                );
            }
        }
    }

    /// Byzantine behaviour: react to a message with conflicting votes and proposals.
    fn misbehave(&mut self, me: usize, msg: &SignedMessage) {
        let round = msg.message.round();
        let n = u64::try_from(self.scenario.n).unwrap();
        let last = self.byz_rounds.get(&me).copied();
        if last.is_none_or(|l| round > l) {
            self.byz_rounds.insert(me, round);
            // As leader of a new round: conflicting proposals to the two halves.
            if round % n == u64::try_from(me).unwrap() {
                let a = self.views[me].best();
                let b = self.random_known(me);
                for (target, half) in [(a, 0usize), (b, 1)] {
                    let proposal = Rc::new(self.signed(
                        me,
                        Message::Proposal {
                            round,
                            target,
                            justify: None,
                        },
                    ));
                    let split = move |to: usize| to % 2 == half;
                    self.broadcast(me, &proposal, Some(&split));
                }
            }
        }
        if let Some(target) = msg.message.target()
            && self.byz_seen.insert((me, round, target))
        {
            let other = self.random_known(me);
            for t in [target, other] {
                for kind in [VoteKind::Prepare, VoteKind::Commit] {
                    let vote = Rc::new(self.signed(
                        me,
                        Message::Vote {
                            kind,
                            round,
                            target: t,
                        },
                    ));
                    let half = usize::try_from(self.rng.below(2)).unwrap();
                    let split = move |to: usize| to % 2 == half || t == target;
                    self.broadcast(me, &vote, Some(&split));
                }
            }
        }
    }

    /// A random recent block this node knows, fork siblings included, to vote for conflicting
    /// targets.
    fn random_known(&mut self, me: usize) -> BlockRef {
        let view = &self.views[me];
        let floor = view.best().number.saturating_sub(4);
        let recent: Vec<BlockRef> = view
            .known
            .iter()
            .filter_map(|h| view.block(*h))
            .filter(|b| b.number >= floor)
            .collect();
        recent[usize::try_from(self.rng.below(u64::try_from(recent.len()).unwrap())).unwrap()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeds per size; three sizes give 1002 runs in total (task 2.7).
    const SEEDS_PER_SIZE: u64 = 334;

    fn seeds_env() -> u64 {
        std::env::var("AC_BFT_SIM_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(SEEDS_PER_SIZE)
    }

    // consensus/ac-bft Scenarios "模拟网络下的安全性" and "模拟网络下的活性": 4, 7 and 10
    // members, up to f Byzantine, random delays, losses and reordering before GST. Honest nodes
    // never finalize conflicting blocks, and after GST every honest node finalizes new blocks.
    #[test]
    fn safety_and_liveness_under_random_faults() {
        let seeds = seeds_env();
        for n in [4usize, 7, 10] {
            for seed in 0..seeds {
                let mut rng = TestRng::new(seed.wrapping_mul(7919).wrapping_add(n as u64));
                let scenario = Scenario::random(n, &mut rng);
                let outcome = run(&scenario, seed);
                outcome.assert_safe();
                assert!(
                    outcome.all_progressed_after_gst(),
                    "no progress after GST: n={n} seed={seed} {scenario:?}\n{}",
                    outcome.states.join("\n")
                );
            }
        }
    }

    // A fully honest, calm network finalizes almost every block within a few hundred ms.
    #[test]
    fn calm_network_finalizes_every_slot() {
        let scenario = Scenario {
            n: 4,
            byzantine: Vec::new(),
            slot_ms: 1000,
            gst_ms: 0,
            chaos_delay_ms: 0,
            drop_percent: 0,
            calm_delay_ms: 50,
            duration_ms: 30_500,
        };
        let outcome = run(&scenario, 1);
        outcome.assert_safe();
        for blocks in outcome.finalized.values() {
            assert!(blocks.last().unwrap().number >= 29, "{blocks:?}");
        }
    }

    // Byzantine members that equivocate are reported by honest ones.
    #[test]
    fn equivocators_are_reported() {
        let scenario = Scenario {
            n: 4,
            byzantine: vec![(2, Byzantine::Equivocate)],
            slot_ms: 1000,
            gst_ms: 0,
            chaos_delay_ms: 0,
            drop_percent: 0,
            calm_delay_ms: 50,
            duration_ms: 20_000,
        };
        let outcome = run(&scenario, 3);
        outcome.assert_safe();
        assert!(outcome.reports > 0);
        assert!(outcome.all_progressed_after_gst());
    }

    proptest::proptest! {
        // Requirement "安全性：不会最终确定冲突区块": arbitrary sizes, Byzantine strategies (up to
        // f) and network conditions never break safety.
        #[test]
        fn safety_for_arbitrary_faults(
            n in 4usize..=10,
            seed in proptest::prelude::any::<u64>(),
            byzantine_bits in proptest::collection::vec(proptest::prelude::any::<bool>(), 3),
            gst_ms in 0u64..20_000,
            chaos_delay_ms in 0u64..4_000,
            drop_percent in 0u64..60,
        ) {
            let f = (n - 1) / 3;
            let byzantine = byzantine_bits
                .iter()
                .take(f)
                .enumerate()
                .map(|(i, equivocate)| {
                    let kind = if *equivocate { Byzantine::Equivocate } else { Byzantine::Silent };
                    // Spread faulty members over the set.
                    ((i * 3 + usize::try_from(seed % 3).unwrap()) % n, kind)
                })
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect();
            let scenario = Scenario {
                n,
                byzantine,
                slot_ms: 1000,
                gst_ms,
                chaos_delay_ms,
                drop_percent,
                calm_delay_ms: 100,
                duration_ms: 30_000,
            };
            run(&scenario, seed).assert_safe();
        }
    }
}
