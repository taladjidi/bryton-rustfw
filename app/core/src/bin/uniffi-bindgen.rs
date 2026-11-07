//! UniFFI's own command line, run by app/ios/build-core.sh to write the Swift bindings.

fn main() {
    uniffi::uniffi_bindgen_main()
}
