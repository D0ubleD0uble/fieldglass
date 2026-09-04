//! Emits the linker configuration a Node.js native addon needs, via
//! `napi_build::setup()`. Nothing else; the bindings themselves are generated
//! by the `#[napi]` attribute macros in `src/lib.rs`.

extern crate napi_build;

fn main() {
    napi_build::setup();
}
