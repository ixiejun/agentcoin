//! Test only: the AgentCoin runtime compiled with an over-minting fault.
//!
//! [`WASM_BINARY`] is the runtime built with `--cfg ac_test_overmint`: each emission settlement
//! mints twice the scheduled amount more than the formula allows, and its `spec_version` is one
//! higher than the real runtime's. Node tests upgrade a development chain to it through the PoA
//! multisig and check that the node rejects the first settlement block (spec node/invariants
//! "超发的 runtime 升级"). Never use it anywhere else.

#![cfg_attr(not(feature = "std"), no_std)]

// In the WASM build this crate only wraps the runtime, whose entry points it re-exports.
#[cfg(not(feature = "std"))]
pub use ac_runtime::*;

// The generated constants (`WASM_BINARY`, `WASM_BINARY_BLOATY`) carry no docs.
#[cfg(feature = "std")]
#[allow(missing_docs)]
mod blob {
    include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));
}
#[cfg(feature = "std")]
pub use blob::{WASM_BINARY, WASM_BINARY_BLOATY, WASM_BINARY_PATH};

#[cfg(all(test, feature = "std"))]
mod tests {
    #![allow(clippy::unwrap_used)]

    // The over-minting blob is a separate WASM build: it exists, is a WASM module and is not the
    // real runtime's blob.
    #[test]
    fn blob_is_a_distinct_runtime() {
        let faulty = super::WASM_BINARY.unwrap();
        let real = ac_runtime::WASM_BINARY.unwrap();
        assert!(faulty.starts_with(b"\0asm"));
        assert_ne!(faulty, real);
        assert!(
            super::WASM_BINARY_PATH
                .unwrap()
                .contains("wbuild/ac-overmint-runtime/")
        );
    }
}
