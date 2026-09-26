//! Builds the WASM runtime blob when compiled with `std`.

#[cfg(feature = "std")]
fn main() {
    substrate_wasm_builder::WasmBuilder::build_using_defaults();
}

// The WASM builder is not needed when this crate itself is compiled to WASM.
#[cfg(not(feature = "std"))]
fn main() {}
