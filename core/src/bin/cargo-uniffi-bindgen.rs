// In-project UniFFI CLI (uniffi 0.28 library mode).
//
// The published `uniffi` crate does not ship a CLI binary; the documented
// pattern is a small binary target in the crate that calls
// `uniffi::uniffi_bindgen_main()`. Named `cargo-uniffi-bindgen` so that
// `cargo uniffi-bindgen <args>` resolves it as an in-project cargo subcommand
// once it has been built into the target directory.

fn main() {
    uniffi::uniffi_bindgen_main();
}
