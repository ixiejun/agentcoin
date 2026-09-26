//! The chain's cryptographic profile, published through a runtime API so that clients can check
//! they are talking to a chain that hashes the way they expect.

use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// Hash function used for block hashes, the extrinsics root and the state trie.
///
/// Wire-format enum: discriminants are explicit and never reused (AGENT.md §5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
#[repr(u8)]
pub enum ChainHashing {
    /// BLAKE3 with a 32-byte output (decision D35).
    #[codec(index = 1)]
    Blake3_256 = 1,
}

/// Cryptographic profile of the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct ChainProfile {
    /// Hash function behind every on-chain integrity commitment.
    pub hashing: ChainHashing,
}

impl ChainProfile {
    /// The profile of every AgentCoin chain built from this code base.
    pub const AGENTCOIN: Self = Self {
        hashing: ChainHashing::Blake3_256,
    };
}

sp_api::decl_runtime_apis! {
    /// Publishes the chain's cryptographic profile.
    pub trait ChainProfileApi {
        /// The chain's cryptographic profile.
        fn profile() -> ChainProfile;
    }
}
