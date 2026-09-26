//! Validation and flooding of AC-BFT messages (design D3 of `m2-finality`).
//!
//! [`MessageFilter`] decides what to do with every message received from a peer: accept and
//! relay it, ignore a duplicate or out-of-window message, hold a message of the next authority
//! set until the local node switches, or reject an invalid one and lower the sender's
//! reputation. It has no networking of its own, so it is tested in memory; the gadget feeds it
//! the notification stream.

use std::collections::{HashSet, VecDeque};

use ac_primitives::ac_bft::{
    Authority, Round, SetId, SignedMessage, VersionedMessage, verify_message,
};
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;

use crate::protocol::{FUTURE_ROUNDS, PAST_ROUNDS};

/// Notification protocol name suffix; the full name is `/<genesis hex>/acbft/1`.
pub const PROTOCOL_SUFFIX: &str = "acbft/1";
/// Maximum size of one notification.
pub const MAX_MESSAGE_SIZE: u64 = 256 * 1024;
/// Number of message hashes remembered to drop duplicates.
pub const SEEN_CAPACITY: usize = 16_384;
/// Messages of the next set held until the node switches to it.
pub const HELD_CAPACITY: usize = 1_024;

/// The protocol name for a chain.
#[must_use]
pub fn protocol_name(genesis: &H256) -> String {
    format!("/{}/{PROTOCOL_SUFFIX}", hex::encode(genesis.as_bytes()))
}

/// What to do with a received message.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Valid and new: hand to the voter and relay to every other peer.
    Accept(SignedMessage),
    /// Seen before: ignore.
    Duplicate,
    /// Belongs to the next set; held and returned by [`MessageFilter::set_changed`].
    Held,
    /// Outside the round window or of an old set: drop without penalty.
    Stale,
    /// Malformed, wrongly signed, from a non-member or another chain: drop and lower the
    /// sender's reputation.
    Invalid,
}

/// Bounded set of recently seen message hashes.
#[derive(Debug, Default)]
struct Seen {
    order: VecDeque<H256>,
    set: HashSet<H256>,
}

impl Seen {
    /// Inserts `hash`; returns `false` if it was already present.
    fn insert(&mut self, hash: H256) -> bool {
        if !self.set.insert(hash) {
            return false;
        }
        self.order.push_back(hash);
        if self.order.len() > SEEN_CAPACITY
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

/// Validates incoming messages for the node's current authority set.
#[derive(Debug)]
pub struct MessageFilter {
    genesis: H256,
    set_id: SetId,
    authorities: Vec<Authority>,
    round: Round,
    seen: Seen,
    held: VecDeque<Vec<u8>>,
}

impl MessageFilter {
    /// A filter for set `set_id` of the chain `genesis`.
    #[must_use]
    pub fn new(genesis: H256, set_id: SetId, authorities: Vec<Authority>) -> Self {
        Self {
            genesis,
            set_id,
            authorities,
            round: 0,
            seen: Seen::default(),
            held: VecDeque::new(),
        }
    }

    /// Updates the voter's current round (defines the accepted window).
    pub fn set_round(&mut self, round: Round) {
        self.round = round;
    }

    /// Switches to a new set and returns the held messages of that set, to be checked again.
    pub fn set_changed(&mut self, set_id: SetId, authorities: Vec<Authority>) -> Vec<Vec<u8>> {
        self.set_id = set_id;
        self.authorities = authorities;
        self.round = 0;
        self.held.drain(..).collect()
    }

    /// Encodes a local message for the wire and records it as seen.
    pub fn outgoing(&mut self, message: &SignedMessage) -> Vec<u8> {
        let bytes = VersionedMessage::V1(message.clone()).encode();
        self.seen.insert(H256(ac_crypto::hash::blake3_256(&bytes)));
        bytes
    }

    /// Checks a notification received from a peer.
    pub fn incoming(&mut self, bytes: &[u8]) -> Verdict {
        if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_MESSAGE_SIZE) {
            return Verdict::Invalid;
        }
        let hash = H256(ac_crypto::hash::blake3_256(bytes));
        if !self.seen.insert(hash) {
            return Verdict::Duplicate;
        }
        let mut input = bytes;
        let Ok(VersionedMessage::V1(message)) = VersionedMessage::decode(&mut input) else {
            return Verdict::Invalid;
        };
        if !input.is_empty() {
            return Verdict::Invalid;
        }
        if message.set_id < self.set_id {
            return Verdict::Stale;
        }
        if message.set_id == self.set_id.saturating_add(1) {
            // The next set's messages can only be checked after the switch.
            if self.held.len() >= HELD_CAPACITY {
                self.held.pop_front();
            }
            self.held.push_back(bytes.to_vec());
            return Verdict::Held;
        }
        if message.set_id != self.set_id {
            return Verdict::Stale;
        }
        let round = message.message.round();
        if round.saturating_add(PAST_ROUNDS) < self.round
            || round > self.round.saturating_add(FUTURE_ROUNDS)
        {
            return Verdict::Stale;
        }
        match verify_message(&self.genesis, self.set_id, &self.authorities, &message) {
            Ok(()) => Verdict::Accept(message),
            Err(_) => Verdict::Invalid,
        }
    }

    /// Re-checks a held message after a set change without treating it as a duplicate.
    pub fn recheck(&mut self, bytes: &[u8]) -> Verdict {
        let hash = H256(ac_crypto::hash::blake3_256(bytes));
        self.seen.set.remove(&hash);
        self.incoming(bytes)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use ac_crypto::sig::SigningKey;
    use ac_crypto::{SigAlg, dev_seed};
    use ac_primitives::ac_bft::{BlockRef, Message, VOTE_CONTEXT, VoteKind, signing_payload};

    const NAMES: [&str; 4] = ["alice", "bob", "charlie", "dave"];

    fn keys() -> Vec<SigningKey> {
        NAMES
            .iter()
            .map(|n| SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n).unwrap()).unwrap())
            .collect()
    }

    fn set(keys: &[SigningKey]) -> Vec<Authority> {
        keys.iter()
            .map(|k| Authority::poa(k.public_key().unwrap()))
            .collect()
    }

    fn vote(
        (key, index): (&SigningKey, u16),
        genesis: &H256,
        set_id: SetId,
        round: Round,
        target: u8,
    ) -> SignedMessage {
        let message = Message::Vote {
            kind: VoteKind::Prepare,
            round,
            target: BlockRef {
                hash: H256::repeat_byte(target),
                number: 1,
            },
        };
        let payload = signing_payload(genesis, set_id, &message).unwrap();
        SignedMessage {
            set_id,
            signer: index,
            message,
            signature: key.sign_deterministic(&payload, VOTE_CONTEXT).unwrap(),
        }
    }

    // Scenario "有效投票被接受和转发": a valid message is accepted by the first handler, relayed,
    // and accepted by the second; the relay back is a duplicate.
    #[test]
    fn valid_message_is_accepted_and_relayed() {
        let keys = keys();
        let genesis = H256::repeat_byte(1);
        let mut a = MessageFilter::new(genesis, 0, set(&keys));
        let mut b = MessageFilter::new(genesis, 0, set(&keys));
        let msg = vote((&keys[2], 2), &genesis, 0, 0, 5);
        let wire = VersionedMessage::V1(msg.clone()).encode();
        assert_eq!(a.incoming(&wire), Verdict::Accept(msg.clone()));
        assert_eq!(b.incoming(&wire), Verdict::Accept(msg));
        assert_eq!(a.incoming(&wire), Verdict::Duplicate);
    }

    // Scenario "篡改的投票被丢弃".
    #[test]
    fn tampered_message_is_invalid() {
        let keys = keys();
        let genesis = H256::repeat_byte(1);
        let mut filter = MessageFilter::new(genesis, 0, set(&keys));
        let mut msg = vote((&keys[2], 2), &genesis, 0, 0, 5);
        msg.message = Message::Vote {
            kind: VoteKind::Prepare,
            round: 0,
            target: BlockRef {
                hash: H256::repeat_byte(6),
                number: 1,
            },
        };
        assert_eq!(
            filter.incoming(&VersionedMessage::V1(msg).encode()),
            Verdict::Invalid
        );
        assert_eq!(filter.incoming(&[1, 2, 3]), Verdict::Invalid);
        let mut trailing = VersionedMessage::V1(vote((&keys[1], 1), &genesis, 0, 0, 5)).encode();
        trailing.push(0);
        assert_eq!(filter.incoming(&trailing), Verdict::Invalid);
    }

    // Scenario "其他链的投票无效".
    #[test]
    fn other_chain_message_is_invalid() {
        let keys = keys();
        let mut filter = MessageFilter::new(H256::repeat_byte(1), 0, set(&keys));
        let foreign = vote((&keys[0], 0), &H256::repeat_byte(9), 0, 0, 5);
        assert_eq!(
            filter.incoming(&VersionedMessage::V1(foreign).encode()),
            Verdict::Invalid
        );
    }

    // Round window, old sets, and next-set messages held until the switch.
    #[test]
    fn windows_and_set_changes() {
        let keys = keys();
        let genesis = H256::repeat_byte(1);
        let mut filter = MessageFilter::new(genesis, 1, set(&keys));
        filter.set_round(100);
        let old_round = vote((&keys[0], 0), &genesis, 1, 100 - PAST_ROUNDS - 1, 5);
        let future_round = vote((&keys[0], 0), &genesis, 1, 100 + FUTURE_ROUNDS + 1, 5);
        let old_set = vote((&keys[0], 0), &genesis, 0, 100, 5);
        let next_set = vote((&keys[1], 0), &genesis, 2, 0, 5);
        for m in [old_round, future_round, old_set] {
            assert_eq!(
                filter.incoming(&VersionedMessage::V1(m).encode()),
                Verdict::Stale
            );
        }
        let wire = VersionedMessage::V1(next_set.clone()).encode();
        assert_eq!(filter.incoming(&wire), Verdict::Held);
        let held = filter.set_changed(2, set(&keys[1..]));
        assert_eq!(held.len(), 1);
        assert_eq!(filter.recheck(&held[0]), Verdict::Accept(next_set));
    }

    // Local messages are marked as seen, so relays of them are duplicates.
    #[test]
    fn outgoing_messages_are_not_reaccepted() {
        let keys = keys();
        let genesis = H256::repeat_byte(1);
        let mut filter = MessageFilter::new(genesis, 0, set(&keys));
        let msg = vote((&keys[0], 0), &genesis, 0, 0, 5);
        let wire = filter.outgoing(&msg);
        assert_eq!(filter.incoming(&wire), Verdict::Duplicate);
        assert_eq!(
            protocol_name(&H256::zero()),
            format!("/{}/acbft/1", "00".repeat(32))
        );
    }
}
