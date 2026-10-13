//! WebAssembly frontend for Glassbit.
//!
//! Phase 0 only proves the wasm pipeline builds; the canvas, input and audio
//! glue arrive in Phase 5.

use wasm_bindgen::prelude::*;

/// Version of the emulator core this frontend was built against.
#[wasm_bindgen]
pub fn core_version() -> String {
    glassbit_core::VERSION.to_string()
}
