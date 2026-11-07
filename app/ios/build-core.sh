#!/bin/sh
# Builds the Rust core (app/core) for the iPhone and its simulator, writes the Swift bindings with UniFFI's
# bindgen, and wraps both libraries in an XCFramework. Output (gitignored): app/ios/Generated/.
# Run it before opening or building the Xcode project; Xcode does not run it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
core=$(cd "$here/../core" && pwd)
out="$here/Generated"
name=bryton_app_core
cd "$core"

for t in aarch64-apple-ios aarch64-apple-ios-sim; do
    cargo build --release --target "$t"
done

rm -rf "$out"
mkdir -p "$out/swift" "$out/headers"
# library mode: the bindings are read from the compiled library's metadata
cargo run --release --target aarch64-apple-darwin --bin uniffi-bindgen -- \
    generate --library "target/aarch64-apple-ios-sim/release/lib$name.a" --language swift --out-dir "$out/swift"
mv "$out/swift/${name}FFI.h" "$out/headers/"
mv "$out/swift/${name}FFI.modulemap" "$out/headers/module.modulemap"

xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/lib$name.a" -headers "$out/headers" \
    -library "target/aarch64-apple-ios-sim/release/lib$name.a" -headers "$out/headers" \
    -output "$out/BrytonCore.xcframework"
echo "built $out"
