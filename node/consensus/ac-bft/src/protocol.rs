//! The AC-BFT protocol as a pure, deterministic state machine (design D2 of `m2-finality`).
//!
//! A [`Voter`] runs one authority set. It never touches the network, the database or the
//! clock: callers feed it events (a verified message, a block import, a finalization, the
//! current time) together with a read-only view of the block tree ([`Chain`]) and execute the
//! [`Action`]s it returns **in order**. In particular a [`Action::Persist`] always precedes
//! the [`Action::Broadcast`] it protects, so a crash can never lose the record of a signed
//! message.
//!
//! # Protocol
//!
//! With `n` members of total weight `W`, a certificate needs weight `q = ⌊2W/3⌋ + 1`.
//! 1. The leader of round `r` (member `r mod n`) proposes its best block, capped at the first
//!    unfinalized authority-set change, extending the highest prepare certificate it has seen.
//! 2. Members prepare-vote the proposal if it extends the last finalized block and passes the
//!    **locking rule**: no lock, or the target extends the locked block, or the proposal
//!    justifies itself with a prepare certificate of a round above the lock.
//! 3. On a prepare certificate for the current round, members commit-vote, lock on it and
//!    move to the next round at once (the next prepare phase runs alongside this commit
//!    phase).
//! 4. A commit certificate finalizes its target.
//!
//! A round without a prepare certificate times out; timeouts back off by ×1.5 up to
//! [`MAX_TIMEOUT_MS`]. `q` timeouts, or messages of a higher round from members holding more
//! than `W − q` weight, move a member forward. After a timeout the next leader waits half a
//! slot for votes still in flight before proposing.
//!
//! # Safety argument
//!
//! Faulty weight is below `W − q + 1`, so any two sets of weight `q` share an honest member
//! (their overlap is at least `2q − W > W − q`). Suppose `T` gets a commit certificate in
//! round `r`, and let `r' > r` be the lowest round with a prepare certificate for a block that
//! does **not** extend `T` (a conflicting block or a strict ancestor of `T`). Its prepare
//! quorum shares an honest member `h` with the commit quorum of `(r, T)`. `h` locked on
//! `(r, T)`; locks only move to higher rounds, and by the choice of `r'` every certificate in
//! `(r, r')` is for a block extending `T`, so at `r'` `h` is locked on a block extending `T`.
//! The target of round `r'` does not extend that block, so `h` prepared it only because the
//! proposal carried a certificate of a round in `(lock round, r')` that the target extends —
//! a certificate for a block not extending `T` in `(r, r')`, contradicting the choice of `r'`.
//! (Justifications must come from an earlier round than the proposal, see [`Voter`].) Two
//! conflicting prepare certificates in round `r` itself would need an honest member to prepare
//! twice. So every later certificate extends `T`, and no two conflicting blocks are ever
//! finalized. The simulator in [`crate::sim`] checks this under random delays, losses and
//! Byzantine behaviour.
#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use ac_crypto::PqSignature;
use ac_primitives::ac_bft::{
    Authority, AuthorityIndex, BlockRef, CertRef, FinalityProof, Message, MessageKind, Round,
    SetId, SignedMessage, VoteKind, threshold, total_weight,
};
use ac_primitives::offences::Evidence;
use parity_scale_codec::{Decode, Encode};
use sp_runtime::BoundedVec;

/// Upper bound of the round timeout.
pub const MAX_TIMEOUT_MS: u64 = 30_000;
/// Messages of rounds this far behind the current round are ignored and their votes dropped.
pub const PAST_ROUNDS: Round = 16;
/// Signed messages are kept this many rounds back to detect double signing. Bounds memory:
/// every kept message carries a 3.3 KB signature.
pub const EVIDENCE_ROUNDS: Round = 8;
/// Messages of rounds this far ahead of the current round are not kept; only the round their
/// signer reached is noted, for catching up.
pub const FUTURE_ROUNDS: Round = 32;

/// Read-only view of the block tree.
pub trait Chain {
    /// Whether the block has been imported.
    fn contains(&self, block: &BlockRef) -> bool;
    /// Whether `block` is `ancestor` or one of its descendants. `false` if either is unknown.
    fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool;
    /// The current best block. Like the node's fork choice, it must descend from the last
    /// finalized block: finalized blocks are never reverted.
    fn best(&self) -> BlockRef;
    /// The first block after `after` (exclusive) up to `upto` (inclusive), on the chain ending
    /// at `upto`, that announces an authority-set change.
    fn first_change(&self, after: &BlockRef, upto: &BlockRef) -> Option<BlockRef>;
}

/// Static parameters of one authority set.
#[derive(Clone, Debug)]
pub struct Config {
    /// Authority-set id.
    pub set_id: SetId,
    /// Members in order.
    pub authorities: Vec<Authority>,
    /// This node's index in the set; `None` for a node that only follows finality.
    pub me: Option<AuthorityIndex>,
    /// Aura slot duration; the base round timeout is two slots.
    pub slot_ms: u64,
}

/// What must survive a restart so that a member never signs two different messages of one
/// kind in one round. Written before every broadcast.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct VoterState {
    /// Authority set the state belongs to.
    pub set_id: SetId,
    /// Round the member was in.
    pub round: Round,
    /// Lock: the prepare certificate of the member's latest commit vote.
    pub lock: Option<CertRef>,
    /// Last message signed per kind, with its round.
    pub signed: Vec<(MessageKind, Round, Message)>,
}

/// Something the caller must do, in the order returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Durably store the voter state before continuing.
    Persist(VoterState),
    /// Sign the message with this node's key and send it to every peer, then feed the signed
    /// message back into [`Voter::on_message`].
    Broadcast(Message),
    /// Finalize the proof's target (and its ancestors) and keep the proof.
    Finalize(FinalityProof),
    /// Submit double-signing evidence on chain.
    Report(Box<Evidence>),
    /// Send these already-signed messages (another member's certificate) to every peer. Used to
    /// pull a lagging peer into the current round.
    Relay(Vec<SignedMessage>),
}

/// State of the current round that is not persisted.
#[derive(Clone, Debug, Default)]
struct RoundState {
    via_timeout: bool,
    /// After a timeout, the leader waits until then for votes still in flight.
    leader_wait_until: Option<u64>,
    proposed: bool,
    prepared: bool,
    timed_out: bool,
}

/// An AC-BFT participant for one authority set.
#[derive(Debug)]
pub struct Voter {
    config: Config,
    quorum: u128,
    /// Smallest weight that must contain an honest member: `W − q + 1`.
    honest: u128,
    state: VoterState,
    round: RoundState,
    finalized: BlockRef,
    high: Option<CertRef>,
    deadline: Option<u64>,
    failures: u32,
    votes: BTreeMap<(Round, VoteKind, BlockRef), BTreeMap<AuthorityIndex, PqSignature>>,
    prepare_certs: BTreeMap<Round, BlockRef>,
    timeouts: BTreeMap<Round, BTreeMap<AuthorityIndex, SignedMessage>>,
    /// The certificate (q timeouts or q prepare votes) that moved this voter into its current
    /// round, relayed to peers still in an earlier round.
    entry_certificate: Vec<SignedMessage>,
    /// Prepare votes of the highest certificate, relayed to peers whose timeouts show a lower
    /// one.
    high_certificate: Vec<SignedMessage>,
    last_relay: BTreeMap<AuthorityIndex, u64>,
    proposals: BTreeMap<Round, (BlockRef, Option<CertRef>)>,
    seen: BTreeMap<(AuthorityIndex, Round, MessageKind), SignedMessage>,
    reported: BTreeSet<(AuthorityIndex, Round, MessageKind)>,
    max_round: BTreeMap<AuthorityIndex, Round>,
    pending_commit: Option<(Round, BlockRef)>,
    conflicts: u64,
}

impl Voter {
    /// Starts a voter for `config`'s set with `finalized` as the last finalized block,
    /// resuming from `persisted` if it belongs to the same set.
    #[must_use]
    pub fn new(config: Config, persisted: Option<VoterState>, finalized: BlockRef) -> Self {
        let total = total_weight(&config.authorities).unwrap_or(0);
        let quorum = threshold(total);
        let honest = total.saturating_sub(quorum).saturating_add(1);
        let state = persisted
            .filter(|s| s.set_id == config.set_id)
            .unwrap_or(VoterState {
                set_id: config.set_id,
                ..VoterState::default()
            });
        Self {
            config,
            quorum,
            honest,
            state,
            round: RoundState::default(),
            finalized,
            high: None,
            deadline: None,
            failures: 0,
            votes: BTreeMap::new(),
            prepare_certs: BTreeMap::new(),
            timeouts: BTreeMap::new(),
            entry_certificate: Vec::new(),
            high_certificate: Vec::new(),
            last_relay: BTreeMap::new(),
            proposals: BTreeMap::new(),
            seen: BTreeMap::new(),
            reported: BTreeSet::new(),
            max_round: BTreeMap::new(),
            pending_commit: None,
            conflicts: 0,
        }
    }

    /// Current round.
    #[must_use]
    pub fn round(&self) -> Round {
        self.state.round
    }

    /// Last finalized block known to the voter.
    #[must_use]
    pub fn finalized(&self) -> BlockRef {
        self.finalized
    }

    /// Current lock.
    #[must_use]
    pub fn lock(&self) -> Option<CertRef> {
        self.state.lock
    }

    /// Highest prepare certificate seen.
    #[must_use]
    pub fn high(&self) -> Option<CertRef> {
        self.high
    }

    /// Commit certificates that conflicted with the finalized chain. Always 0 unless more than
    /// a third of the weight is faulty; the simulator asserts it.
    #[must_use]
    pub fn conflicts(&self) -> u64 {
        self.conflicts
    }

    /// One-line summary of the round state, for simulator diagnostics.
    #[cfg(any(test, feature = "test-utils"))]
    #[must_use]
    pub fn debug_round(&self) -> String {
        format!(
            "{:?} proposals {:?} certs {:?} timeouts {:?} failures {}",
            self.round,
            self.proposals
                .iter()
                .map(|(r, (t, j))| (*r, t.number, j.map(|c| c.round)))
                .collect::<Vec<_>>(),
            self.prepare_certs
                .iter()
                .map(|(r, t)| (*r, t.number))
                .collect::<Vec<_>>(),
            self.timeouts
                .iter()
                .map(|(r, s)| (*r, s.len()))
                .collect::<Vec<_>>(),
            self.failures,
        )
    }

    /// Authority set of this voter.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// When [`Voter::on_tick`] should next be called, if ever.
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
        match (self.deadline, self.round.leader_wait_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Leader of `round`: member `round mod n`.
    #[must_use]
    pub fn leader(&self, round: Round) -> Option<AuthorityIndex> {
        let n = u64::try_from(self.config.authorities.len()).ok()?;
        AuthorityIndex::try_from(round.checked_rem(n)?).ok()
    }

    /// Round timeout after `failures` consecutive rounds without finality: two slots, ×1.5 per
    /// failure, at most [`MAX_TIMEOUT_MS`].
    #[must_use]
    pub fn timeout_ms(&self, failures: u32) -> u64 {
        let mut t = self.config.slot_ms.saturating_mul(2);
        for _ in 0..failures {
            t = t.saturating_mul(3).checked_div(2).unwrap_or(t);
            if t >= MAX_TIMEOUT_MS {
                return MAX_TIMEOUT_MS;
            }
        }
        t.min(MAX_TIMEOUT_MS)
    }

    /// Handles a message whose signature and set membership the caller already verified.
    pub fn on_message(
        &mut self,
        signed: SignedMessage,
        chain: &impl Chain,
        now: u64,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if signed.set_id != self.config.set_id || self.weight_of_index(signed.signer).is_none() {
            return actions;
        }
        let round = signed.message.round();
        if round.saturating_add(PAST_ROUNDS) < self.state.round {
            return actions;
        }
        if round > self.state.round.saturating_add(FUTURE_ROUNDS) {
            // Too far ahead to keep, but it shows how far its signer got: a member that fell far
            // behind (a new node, a long outage) catches up once members worth more than `W − q`
            // are seen ahead. Only the round is recorded, so memory stays bounded.
            self.note_round(signed.signer, round);
            let before = self.state.round;
            self.catch_up(chain, now);
            if round > self.state.round.saturating_add(FUTURE_ROUNDS) {
                if self.state.round > before {
                    self.progress(chain, now, &mut actions);
                }
                return actions;
            }
        }
        self.help_lagging_peer(signed.signer, round, now, &mut actions);
        let key = (signed.signer, round, signed.message.kind());
        match self.seen.get(&key) {
            Some(first) if first.message == signed.message => return actions,
            Some(first) => {
                if key.2 != MessageKind::Timeout && self.reported.insert(key) {
                    actions.push(Action::Report(Box::new(Evidence::BftEquivocation {
                        first: Box::new(first.clone()),
                        second: Box::new(signed.clone()),
                    })));
                }
                // A double vote still counts towards its own target (faulty weight may appear in
                // two certificates; the safety bound allows for it). Only the first proposal or
                // timeout of a member counts.
                if !matches!(signed.message, Message::Vote { .. }) {
                    return actions;
                }
            }
            None => {
                self.seen.insert(key, signed.clone());
            }
        }
        self.note_round(signed.signer, round);

        match signed.message {
            Message::Proposal {
                round,
                target,
                justify,
            } => {
                if self.leader(round) == Some(signed.signer) {
                    self.proposals.entry(round).or_insert((target, justify));
                }
            }
            Message::Vote {
                kind,
                round,
                target,
            } => {
                let voters = self.votes.entry((round, kind, target)).or_default();
                voters.insert(signed.signer, signed.signature);
                let weight = weight_of(&self.config.authorities, voters.keys());
                if weight >= self.quorum {
                    match kind {
                        VoteKind::Prepare => {
                            if let std::collections::btree_map::Entry::Vacant(e) =
                                self.prepare_certs.entry(round)
                            {
                                e.insert(target);
                                let certificate = self.prepare_certificate(round, target);
                                let before = self.state.round;
                                self.on_prepare_cert(
                                    CertRef { round, target },
                                    chain,
                                    now,
                                    &mut actions,
                                );
                                if self.state.round > before {
                                    self.entry_certificate.clone_from(&certificate);
                                }
                                if self.high.is_some_and(|h| h.round == round) {
                                    self.high_certificate = certificate;
                                }
                            }
                        }
                        VoteKind::Commit => self.on_commit_cert(round, target, chain, &mut actions),
                    }
                }
            }
            Message::Timeout { round, high } => {
                if self
                    .high
                    .is_some_and(|mine| high.is_none_or(|theirs| theirs.round < mine.round))
                    && !self.high_certificate.is_empty()
                    && self.may_relay_to(signed.signer, now)
                {
                    actions.push(Action::Relay(self.high_certificate.clone()));
                }
                let senders = self.timeouts.entry(round).or_default();
                senders.insert(signed.signer, signed.clone());
                let weight = weight_of(&self.config.authorities, senders.keys());
                if weight >= self.quorum && round >= self.state.round {
                    self.entry_certificate = senders.values().cloned().collect();
                    self.enter_round(round.saturating_add(1), true, chain, now);
                }
            }
        }
        self.catch_up(chain, now);
        self.progress(chain, now, &mut actions);
        actions
    }

    /// Handles a newly imported block (the best block may have changed).
    pub fn on_block_imported(&mut self, chain: &impl Chain, now: u64) -> Vec<Action> {
        let mut actions = Vec::new();
        if let Some((round, target)) = self.pending_commit
            && chain.contains(&target)
        {
            self.pending_commit = None;
            self.on_commit_cert(round, target, chain, &mut actions);
        }
        self.progress(chain, now, &mut actions);
        actions
    }

    /// Handles a block finalized by other means (an imported finality proof).
    pub fn on_finalized(&mut self, block: BlockRef, chain: &impl Chain, now: u64) -> Vec<Action> {
        if block.number > self.finalized.number && chain.is_descendant(&block, &self.finalized) {
            self.finalized = block;
            self.failures = 0;
            self.prune();
        }
        let mut actions = Vec::new();
        self.progress(chain, now, &mut actions);
        actions
    }

    /// Handles the passage of time; call at [`Voter::next_deadline`].
    pub fn on_tick(&mut self, chain: &impl Chain, now: u64) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.deadline.is_some_and(|d| now >= d) {
            self.deadline = None;
            if self.config.me.is_some() {
                let message = Message::Timeout {
                    round: self.state.round,
                    high: self.high,
                };
                self.round.timed_out = true;
                self.sign(message, &mut actions);
            }
            // Keep re-sending the same timeout until the round moves on, in case it was lost.
            if self.has_candidate(chain) {
                self.deadline = Some(now.saturating_add(self.timeout_ms(self.failures)));
            }
        }
        self.progress(chain, now, &mut actions);
        actions
    }

    fn weight_of_index(&self, index: AuthorityIndex) -> Option<u128> {
        self.config
            .authorities
            .get(usize::from(index))
            .map(|a| u128::from(a.weight))
    }

    /// A peer still sends messages of an earlier round: relay the certificate that moved this
    /// voter on.
    fn help_lagging_peer(
        &mut self,
        peer: AuthorityIndex,
        round: Round,
        now: u64,
        actions: &mut Vec<Action>,
    ) {
        if round < self.state.round
            && !self.entry_certificate.is_empty()
            && self.may_relay_to(peer, now)
        {
            actions.push(Action::Relay(self.entry_certificate.clone()));
        }
    }

    /// Rate limit of relays: at most once per base timeout per peer, never to ourselves.
    fn may_relay_to(&mut self, peer: AuthorityIndex, now: u64) -> bool {
        if Some(peer) == self.config.me {
            return false;
        }
        let interval = self.timeout_ms(0);
        if self
            .last_relay
            .get(&peer)
            .is_some_and(|t| now < t.saturating_add(interval))
        {
            return false;
        }
        self.last_relay.insert(peer, now);
        true
    }

    fn prepare_certificate(&self, round: Round, target: BlockRef) -> Vec<SignedMessage> {
        self.votes
            .get(&(round, VoteKind::Prepare, target))
            .map(|voters| {
                voters
                    .iter()
                    .map(|(signer, signature)| SignedMessage {
                        set_id: self.config.set_id,
                        signer: *signer,
                        message: Message::Vote {
                            kind: VoteKind::Prepare,
                            round,
                            target,
                        },
                        signature: signature.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_leader(&self) -> bool {
        self.config.me.is_some() && self.leader(self.state.round) == self.config.me
    }

    fn has_candidate(&self, chain: &impl Chain) -> bool {
        chain.best().number > self.finalized.number
    }

    /// Signs `message` if no different message of its kind was signed in its round, recording
    /// it (and the lock) before the broadcast.
    fn sign(&mut self, message: Message, actions: &mut Vec<Action>) -> bool {
        if self.config.me.is_none() {
            return false;
        }
        let kind = message.kind();
        let round = message.round();
        match self.state.signed.iter_mut().find(|(k, _, _)| *k == kind) {
            Some((_, r, m)) if *r > round || (*r == round && *m != message) => return false,
            Some(entry) => *entry = (kind, round, message.clone()),
            None => self.state.signed.push((kind, round, message.clone())),
        }
        self.state.round = self.state.round.max(round);
        actions.push(Action::Persist(self.state.clone()));
        actions.push(Action::Broadcast(message));
        true
    }

    fn on_prepare_cert(
        &mut self,
        cert: CertRef,
        chain: &impl Chain,
        now: u64,
        actions: &mut Vec<Action>,
    ) {
        let CertRef { round, target } = cert;
        if self.high.is_none_or(|h| h.round < round) {
            self.high = Some(cert);
        }
        if round == self.state.round
            && !self.round.timed_out
            && self.config.me.is_some()
            && self.state.lock.is_none_or(|l| l.round <= round)
        {
            let previous = self.state.lock;
            self.state.lock = Some(cert);
            let commit = Message::Vote {
                kind: VoteKind::Commit,
                round,
                target,
            };
            if !self.sign(commit, actions) {
                self.state.lock = previous;
            }
        }
        if round >= self.state.round {
            self.enter_round(round.saturating_add(1), false, chain, now);
        }
    }

    fn on_commit_cert(
        &mut self,
        round: Round,
        target: BlockRef,
        chain: &impl Chain,
        actions: &mut Vec<Action>,
    ) {
        if target.number <= self.finalized.number {
            if target != self.finalized && !chain.is_descendant(&self.finalized, &target) {
                self.conflicts = self.conflicts.saturating_add(1);
            }
            return;
        }
        if !chain.contains(&target) {
            self.pending_commit = Some((round, target));
            return;
        }
        if !chain.is_descendant(&target, &self.finalized) {
            self.conflicts = self.conflicts.saturating_add(1);
            return;
        }
        // Never finalize past an unfinalized set change: that change must be finalized by
        // this set first (design D4). Honest members never vote for such a target.
        if chain
            .first_change(&self.finalized, &target)
            .is_some_and(|c| c != target)
        {
            return;
        }
        let Some(voters) = self.votes.get(&(round, VoteKind::Commit, target)) else {
            return;
        };
        let commits: Vec<_> = voters.iter().map(|(i, s)| (*i, s.clone())).collect();
        let Ok(commits) = BoundedVec::try_from(commits) else {
            return;
        };
        self.finalized = target;
        self.failures = 0;
        actions.push(Action::Finalize(FinalityProof {
            set_id: self.config.set_id,
            round,
            target,
            commits,
        }));
        self.prune();
        if self.deadline.is_some() && !self.has_candidate(chain) {
            self.deadline = None;
        }
    }

    fn enter_round(&mut self, round: Round, via_timeout: bool, chain: &impl Chain, now: u64) {
        if round <= self.state.round {
            return;
        }
        self.state.round = round;
        self.round = RoundState {
            via_timeout,
            leader_wait_until: None,
            ..RoundState::default()
        };
        if via_timeout && self.is_leader() {
            let half_slot = self.config.slot_ms.checked_div(2).unwrap_or(0);
            self.round.leader_wait_until = Some(now.saturating_add(half_slot));
        }
        if via_timeout {
            self.failures = self.failures.saturating_add(1);
        }
        self.deadline = (self.config.me.is_some() && self.has_candidate(chain))
            .then(|| now.saturating_add(self.timeout_ms(self.failures)));
        self.prune();
    }

    /// Records that `signer` reached `round`.
    fn note_round(&mut self, signer: AuthorityIndex, round: Round) {
        let signer_round = self.max_round.entry(signer).or_insert(round);
        *signer_round = (*signer_round).max(round);
    }

    /// Jumps to the highest round that members worth more than `W − q` have reached: at least
    /// one of them is honest, so that round is genuine.
    fn catch_up(&mut self, chain: &impl Chain, now: u64) {
        let mut rounds: Vec<(Round, u128)> = self
            .max_round
            .iter()
            .map(|(i, r)| (*r, self.weight_of_index(*i).unwrap_or(0)))
            .collect();
        rounds.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        let mut weight = 0u128;
        for (round, w) in rounds {
            weight = weight.saturating_add(w);
            if weight >= self.honest {
                if round > self.state.round {
                    self.enter_round(round, true, chain, now);
                }
                return;
            }
        }
    }

    fn progress(&mut self, chain: &impl Chain, now: u64, actions: &mut Vec<Action>) {
        if self.config.me.is_none() {
            return;
        }
        if self.deadline.is_none() && self.has_candidate(chain) && !self.round.timed_out {
            self.deadline = Some(now.saturating_add(self.timeout_ms(self.failures)));
        }
        if self.round.timed_out {
            return;
        }
        if self.round.leader_wait_until.is_some_and(|t| now >= t) {
            self.round.leader_wait_until = None;
        }
        if self.is_leader()
            && !self.round.proposed
            && self.round.leader_wait_until.is_none()
            && let Some(target) = self.proposal_target(chain)
        {
            let proposal = Message::Proposal {
                round: self.state.round,
                target,
                justify: self.high,
            };
            self.round.proposed = self.sign(proposal, actions);
        }
        if !self.round.prepared
            && let Some((target, justify)) = self.proposals.get(&self.state.round).copied()
            && self.may_prepare(&target, justify.as_ref(), chain)
        {
            let vote = Message::Vote {
                kind: VoteKind::Prepare,
                round: self.state.round,
                target,
            };
            self.round.prepared = self.sign(vote, actions);
        }
    }

    /// The block this node would propose, if any.
    fn proposal_target(&self, chain: &impl Chain) -> Option<BlockRef> {
        let best = chain.best();
        let base = self.high.map(|h| h.target);
        let mut candidate = match base {
            Some(b) if !chain.is_descendant(&best, &b) => b,
            _ => best,
        };
        if let Some(change) = chain.first_change(&self.finalized, &candidate) {
            candidate = change;
        }
        let fresh = base.is_none_or(|b| candidate.number > b.number);
        (candidate.number > self.finalized.number
            && chain.contains(&candidate)
            && chain.is_descendant(&candidate, &self.finalized)
            && (fresh || self.round.via_timeout))
            .then_some(candidate)
    }

    fn may_prepare(
        &self,
        target: &BlockRef,
        justify: Option<&CertRef>,
        chain: &impl Chain,
    ) -> bool {
        if !chain.contains(target)
            || target.number <= self.finalized.number
            || !chain.is_descendant(target, &self.finalized)
        {
            return false;
        }
        if chain
            .first_change(&self.finalized, target)
            .is_some_and(|c| c != *target)
        {
            return false;
        }
        match self.state.lock {
            // Extending the lock is always safe; the justification only matters for unlocking.
            None => true,
            Some(lock) if chain.is_descendant(target, &lock.target) => true,
            Some(lock) => justify.is_some_and(|j| {
                // Only certificates of earlier rounds whose votes this node has seen count.
                j.round > lock.round
                    && j.round < self.state.round
                    && self.prepare_certs.get(&j.round) == Some(&j.target)
                    && chain.is_descendant(target, &j.target)
            }),
        }
    }

    fn prune(&mut self) {
        let floor = self.state.round.saturating_sub(PAST_ROUNDS);
        let finalized = self.finalized.number;
        self.votes.retain(|(r, kind, target), _| {
            *r >= floor && (*kind == VoteKind::Prepare || target.number > finalized)
        });
        self.prepare_certs.retain(|r, _| *r >= floor);
        self.timeouts.retain(|r, _| *r >= floor);
        let current = self.state.round;
        self.proposals.retain(|r, _| *r >= current);
        let evidence_floor = self.state.round.saturating_sub(EVIDENCE_ROUNDS);
        self.seen.retain(|(_, r, _), _| *r >= evidence_floor);
        self.reported.retain(|(_, r, _)| *r >= evidence_floor);
    }
}

/// Total weight of `members` (unknown indices count 0).
fn weight_of<'a>(
    authorities: &[Authority],
    members: impl Iterator<Item = &'a AuthorityIndex>,
) -> u128 {
    members.fold(0u128, |acc, i| {
        let w = authorities
            .get(usize::from(*i))
            .map_or(0, |a| u128::from(a.weight));
        acc.saturating_add(w)
    })
}

#[cfg(test)]
mod tests;
