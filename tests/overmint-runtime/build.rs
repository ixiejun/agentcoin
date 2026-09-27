//! Builds the runtime WASM with the test-only over-minting fault (`--cfg ac_test_overmint`):
//! every emission settlement mints twice the scheduled amount more, and `spec_version` is one
//! higher so that `System::set_code` accepts it (m3-economics 8.1).
//!
//! This crate has its own name, so the WASM builder uses its own `wbuild/ac-overmint-runtime`
//! directory and can never overwrite the real runtime blob. The fault is a compiler flag, not a
//! cargo feature, so `--all-features` never enables it anywhere.

#[cfg(feature = "std")]
fn main() {
    substrate_wasm_builder::WasmBuilder::new()
        .with_current_project()
        .export_heap_base()
        .import_memory()
        .append_to_rust_flags("--cfg ac_test_overmint")
        .build();
}

#[cfg(not(feature = "std"))]
fn main() {}
