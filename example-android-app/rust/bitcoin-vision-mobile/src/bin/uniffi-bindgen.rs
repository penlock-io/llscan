//! `cargo run -p bitcoin-vision-mobile --features bindgen --bin uniffi-bindgen --
//! generate --library <built cdylib> --language kotlin` writes the app's
//! Kotlin bindings.

fn main() {
    uniffi::uniffi_bindgen_main()
}
