#!/bin/sh
# Everything to pass before a commit: format, lints, host tests (+ target/sim/*.bmp), firmware build, packing,
# the offline checks of dev.py.
set -e
cd "$(dirname "$0")"
cargo fmt --check
cargo clippy --release -- -D warnings
cargo test-host
if command -v cargo-deny >/dev/null 2>&1; then cargo deny check -s; else echo "cargo-deny: not installed, skipped"; fi
cargo build --release
python3 pack.py
python3 -I dev.py selftest
# The firmware on Renode's STM32H747 (renode/run.py), when Renode is installed (AGENTS.md, Emulation).
export RENODE="${RENODE:-/Applications/Renode.app/Contents/MacOS/renode}"
if command -v "$RENODE" >/dev/null 2>&1; then
    python3 renode/run.py && python3 renode/run.py --vbus
else
    echo "renode: not found, emulator checks skipped (AGENTS.md, Emulation)"
fi
