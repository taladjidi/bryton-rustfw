#!/bin/sh
# Everything to pass before a commit: format, lints, host tests (+ target/sim/*.bmp), firmware build, packing,
# the offline checks of dev.py.
set -e
cd "$(dirname "$0")"
# The split (AGENTS.md, Layout): the library cannot reach hardware, since it forbids unsafe; unsafe lives in hw.rs,
# plus usb.rs's one marker-trait line.
grep -q '^#!\[forbid(unsafe_code)\]' src/lib.rs || { echo "layout: src/lib.rs lost #![forbid(unsafe_code)]"; exit 1; }
n=$(grep -rl 'allow(unsafe_code)\|unsafe {\|unsafe fn\|unsafe impl' src | sort | grep -v '^src/board/hw.rs$' | tr '\n' ' ')
[ "$n" = "src/board/usb.rs " ] && [ "$(grep -c 'unsafe impl\|allow(unsafe_code)' src/board/usb.rs)" = 2 ] \
    || { echo "layout: unsafe outside hw.rs: $n"; exit 1; }
cargo fmt --check
cargo clippy --release -- -D warnings
cargo clippy --tests --target aarch64-apple-darwin -q -- -D warnings  # the tests and the simulator too
cargo test-host
RUSTDOCFLAGS="-D warnings" cargo doc --target aarch64-apple-darwin --no-deps -q  # docs: no broken or private links
if command -v cargo-deny >/dev/null 2>&1; then cargo deny check -s; else echo "cargo-deny: not installed, skipped"; fi
cargo build --release
python3 tools/stack_check.py
python3 pack.py
python3 -I dev.py selftest
python3 -I tools/sync.py selftest  # a fake Intervals.icu and Strava on 127.0.0.1: no network
python3 -I tools/s810_probe.py selftest  # the S810 owner's probe on a made-up volume: nothing personal comes out
python3 -I tools/route_pack.py selftest  # the synthetic map file (test-data/route) built and read back by an independent reader
# the test rides decoded by independent readers (fitdecode, Garmin's FIT SDK): FIT_PYTHON may name a Python that has
# them (a venv); skipped when neither has them
PY="${FIT_PYTHON:-python3}"
if "$PY" -c 'import fitdecode, garmin_fit_sdk' 2>/dev/null; then
    "$PY" tools/fit_check.py target/fit/sample.fit target/fit/cut.fit target/fit/sample_full.fit
    "$PY" tools/fit_check.py --pair target/fit/recorded.fit target/fit/recorded_full.fit
    "$PY" tools/fit_check.py --profile target/fit/indoor.fit target/fit/older.fit
else
    echo "fit_check: fitdecode / garmin-fit-sdk not installed, skipped (FIT_PYTHON)"
fi
# The firmware on Renode's STM32H747 (renode/run.py), when Renode is installed (AGENTS.md, Emulation).
export RENODE="${RENODE:-/Applications/Renode.app/Contents/MacOS/renode}"
if command -v "$RENODE" >/dev/null 2>&1; then
    python3 renode/run.py && python3 renode/run.py --vbus && python3 renode/run.py --off
else
    echo "renode: not found, emulator checks skipped (AGENTS.md, Emulation)"
fi
