#!/bin/sh
# Everything to pass before a commit (AGENTS.md "Before every commit", docs/devloop.md "The gate").
#   ./check.sh          the gate: the guards, then three lanes in parallel (F firmware, H host, P Python tools),
#                       skipping what the branch's diff cannot affect
#   ./check.sh --full   everything, as on main (also rewrites the live numbers)
#   ./check.sh --quick  the iteration tier, NOT the gate: guards, lint, fmt, firmware clippy, host clippy and tests
#   -v                  print every lane's log, not only a failed one
# Each step prints its wall seconds; a lane writes target/check/<lane>.log, shown on failure.
set -e
cd "$(dirname "$0")"
unset WALK_HASHES # a refresh run panics on purpose and must never count as a pass
full=0; quick=0; verbose=0
for a in "$@"; do
    case $a in
        --full) full=1 ;;
        --quick) quick=1 ;;
        -v) verbose=1 ;;
        *) echo "usage: check.sh [--full | --quick] [-v]"; exit 2 ;;
    esac
done
[ $quick = 1 ] || unset FUZZ_SCALE # the gate always runs every fuzz seed
C=target/check
# what the tests wrote (target/fit, target/sim, target/fat) is removed first: a stale file must not pass fit_check,
# fsck or Renode when its test no longer writes it
rm -rf "$C" target/fit target/sim target/fat; mkdir -p "$C"; : > "$C/skipped"; : > "$C/times"
now() { python3 -c 'import time; print(time.time())'; }
t_all=$(now)
# step LABEL CMD...: run, then print the wall seconds (also when it fails); a failure fails the lane (set -e)
step() {
    label=$1; shift
    t0=$(now); rc=0
    "$@" || rc=$?
    t1=$(now)
    awk -v a="$t0" -v b="$t1" -v l="$label" 'BEGIN { printf "[%6.1f s] %s\n", b - a, l }' | tee -a "$C/times"
    return $rc
}
skip() { echo "skip: $1"; echo "$1" >> "$C/skipped"; }

# The split (AGENTS.md, Layout): the library cannot reach hardware, since it forbids unsafe; unsafe lives in hw.rs,
# plus usb.rs's marker-trait line and its allow. Any text "unsafe" in code (comments stripped) outside hw.rs is
# refused, which covers `unsafe {`, `unsafe fn / impl / trait / extern`, `#[unsafe(no_mangle)]`, `allow` / `expect` of
# unsafe_code; the only lines allowed are lib.rs's forbid, main.rs's deny, and those two of usb.rs (exactly).
grep -q '^#!\[forbid(unsafe_code)\]' src/lib.rs || { echo "layout: src/lib.rs lost #![forbid(unsafe_code)]"; exit 1; }
u=$(for f in $(git ls-files -co --exclude-standard src | grep '\.rs$' | grep -v '^src/board/hw\.rs$'); do
        sed 's,//.*,,' "$f" | grep -n 'unsafe' | sed "s,^,$f:,"
    done | grep -vE '^src/lib\.rs:[0-9]+:#!\[forbid\(unsafe_code\)\]|^src/main\.rs:[0-9]+:#!\[deny\(unsafe_code\)\]' || true)
[ "$(printf '%s\n' "$u" | grep -c .)" = 2 ] && printf '%s\n' "$u" | grep -qE '^src/board/usb\.rs:[0-9]+:#\[allow\(unsafe_code\)\]$' \
    && printf '%s\n' "$u" | grep -qE '^src/board/usb\.rs:[0-9]+:unsafe impl UsbPeripheral for ' \
    || { echo "layout: unsafe outside hw.rs (only usb.rs's allow and its one marker impl are allowed):"; printf '%s\n' "$u"; exit 1; }
# The merge guard: no conflict marker left in a tracked text file (one reached main on 2026-10-29 through a
# branch's own merge of main, and nothing else here reads the docs).
m=$(git ls-files -z -- '*.rs' '*.md' '*.py' '*.sh' '*.toml' '*.x' | xargs -0 grep -l '^<<<<<<< \|^>>>>>>> ' 2>/dev/null | tr '\n' ' ')
[ -z "$m" ] || { echo "merge: conflict markers in: $m"; exit 1; }
# The docs guard: every file of src and tools is named in docs/layout.md, by its path or, when no other file has the
# same basename (not mod.rs, tests.rs, fields.rs ...), by its basename as a backticked word (`menus.rs`); the data
# folders (tools/fonts/) are the only ones without an entry of their own.
dup=$(git ls-files -co --exclude-standard src tools | xargs -n1 basename | sort | uniq -d)
# No doc names the files that moved (src/sim.rs, src/ui.rs).
miss=""
for f in $(git ls-files -co --exclude-standard src tools); do
    ok=0
    grep -qF "$f" docs/layout.md && ok=1
    case " $(echo $dup) " in *" $(basename "$f") "*) ;; *) grep -qF "\`$(basename "$f")\`" docs/layout.md && ok=1 ;; esac
    [ "$ok" = 1 ] && continue
    case $f in tools/fonts/*) continue ;; esac
    miss="$miss $f"
done
[ -z "$miss" ] || { echo "docs: not in docs/layout.md:$miss"; exit 1; }
if git grep -nE '(^|[^A-Za-z0-9_])(src/)?(sim|ui)\.rs' -- '*.md'; then echo "docs: a doc names src/sim.rs or src/ui.rs (src/sim/, src/ui/)"; exit 1; fi
# tools/lint.py: pub items only tests use, dashes, licensed keys in tracked files, dead tool functions, ruff (pyflakes)
step "lint.py" python3 -I tools/lint.py > "$C/lint.out" 2>&1 || { cat "$C/lint.out"; exit 1; }
cat "$C/lint.out"
grep -q "keys: no key files" "$C/lint.out" && skip "keys (no key files)"
command -v ruff >/dev/null 2>&1 || skip "ruff (not installed)"
step "cargo fmt --check" cargo fmt --check

# What the branch changed (against the merge base with origin/main, plus the working tree): the skips below.
# On main, without a base, or with --full, everything runs.
changed=""; skipok=0
br=$(git branch --show-current)   # empty on a detached HEAD: treated like main (nothing skipped)
if [ $full = 0 ] && [ -n "$br" ] && [ "$br" != main ]; then
    base=$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || true)
    if [ -n "$base" ]; then
        # shortcut: paths with spaces or newlines are not handled (no -z), none in this repository
        if d=$(git diff --name-only "$base"); then
            changed=$( { printf '%s\n' "$d"; git status --porcelain | cut -c4- | sed 's/.* -> //'; } | sort -u)
            skipok=1
        fi
    fi
fi
# touched REGEX: did a changed path match (always true when nothing may be skipped)
touched() { [ $skipok = 0 ] || printf '%s\n' "$changed" | grep -qE "$1"; }
# touched_beyond REGEX: did a changed path fall outside REGEX (always true when nothing may be skipped)
touched_beyond() { [ $skipok = 0 ] || printf '%s\n' "$changed" | grep -vE "$1" | grep -q .; }
# main, --full and a detached HEAD are strict: a missing optional checker fails instead of skipping
strict=1; [ $skipok = 1 ] && strict=0

if [ $quick = 1 ]; then
    step "cargo clippy --release" cargo clippy --release -- -D warnings  # two branches failed the gate on it
    step "cargo clippy --tests (host)" cargo clippy --tests --target aarch64-apple-darwin -q -- -D warnings
    # Tests: a quarter of the fuzz seeds (FUZZ_SCALE, src/sim/fuzz.rs), and on a branch only the tests the changed paths
    # can affect (tools/test_select.py; ALL when anything is shared or unmapped, or with no diff). The gate runs all.
    filters=ALL
    if [ $skipok = 1 ] && [ -n "$changed" ]; then filters=$(printf '%s\n' "$changed" | python3 -I tools/test_select.py | paste -s -d ' ' -); fi
    echo "quick tests: FUZZ_SCALE=0.25, filters: $filters"
    case $filters in
        ALL) step "cargo test-host" env FUZZ_SCALE="${FUZZ_SCALE:-0.25}" cargo test-host ;;
        NONE) skip "host tests (no Rust, test or build file changed)" ;;
        *)  # a filter that matches no test would pass with 0 tests run: refuse it
            for f in $filters; do
                [ "$(cargo test-host --lib -q -- --list "$f" 2>/dev/null | grep -c ': test$')" -gt 0 ] \
                    || { echo "quick: filter '$f' selects 0 tests (tools/test_select.py is stale)"; exit 1; }
            done
            step "cargo test-host (selected)" env FUZZ_SCALE="${FUZZ_SCALE:-0.25}" cargo test-host --lib -- $filters ;;
    esac
    echo "QUICK TIER: guards, lint, fmt, firmware and host clippy and host tests only (a quarter of the fuzz seeds; the tests the diff can reach). This is NOT the gate: run ./check.sh before a commit."
    exit 0
fi

lane_F() {  # the firmware
    step "cargo clippy --release" cargo clippy --release -- -D warnings
    # the ride build (EMMC_WRITE=1: the recorder and the card writes) is the one that goes on the unit for a ride. It is
    # built first, in the same target dir (option_env! is per crate: only the binary crate rebuilds, the library and
    # the dependencies are shared; measured 35 s against 41 s with a target dir of its own after a lib.rs change),
    # checked, then the default build goes last so that pack.py and the Renode run see the default ELF.
    step "cargo build --release (EMMC_WRITE=1)" env EMMC_WRITE=1 cargo build --release
    step "stack_check (ride build)" python3 -I tools/stack_check.py
    step "panics.py --max 0 (ride build)" python3 -I tools/panics.py --max 0
    step "cargo build --release" cargo build --release
    step "stack_check" python3 -I tools/stack_check.py
    step "stack_check selftest" python3 -I tools/stack_check.py selftest
    step "panics.py --max 0" python3 -I tools/panics.py --max 0
    step "pack.py" python3 -I tools/pack.py
}

lane_H() {  # the host
    step "cargo clippy --tests (host)" cargo clippy --tests --target aarch64-apple-darwin -q -- -D warnings
    step "cargo test-host" cargo test-host
    # the phone app's core (app/core, its own lock file and target dir, built for the Mac)
    step "app/core clippy" sh -c 'cd app/core && cargo clippy --all-targets -q -- -D warnings'
    step "app/core tests" sh -c 'cd app/core && cargo test -q'
    # xcodebuild -version fails on a Command Line Tools install (its shim needs the full Xcode)
    if ! xcodebuild -version >/dev/null 2>&1; then
        skip "app/ios build (no xcodebuild)"
    elif ! touched '^app/'; then
        skip "app/ios build (nothing under app/ changed)"
    elif ! rustup target list --installed 2>/dev/null | grep -q aarch64-apple-ios-sim; then
        skip "app/ios build (no iOS rust targets)"
    else
        step "app/ios build (simulator)" sh -c 'app/ios/build-core.sh >/dev/null && cd app/ios && xcodebuild -project Bryton.xcodeproj -scheme Bryton -sdk iphonesimulator -destination "generic/platform=iOS Simulator" -derivedDataPath build CODE_SIGNING_ALLOWED=NO -quiet build'
    fi
    step "cargo doc" env RUSTDOCFLAGS="-D warnings" cargo doc --target aarch64-apple-darwin --no-deps -q
    # the advisory database is fetched from GitHub: without a network, the cached copy (--offline) is checked instead
    deny() {
        if cargo deny check -s > "$C/deny.out" 2>&1; then cat "$C/deny.out"; return 0; fi
        if [ $strict = 0 ] && grep -q 'failed to fetch advisory database' "$C/deny.out"; then
            skip "cargo-deny advisory database not fetched (no network): checked --offline"
            cargo deny --offline check -s
        else
            cat "$C/deny.out"; return 1
        fi
    }
    if command -v cargo-deny >/dev/null 2>&1; then step "cargo deny" deny; else skip "cargo-deny (not installed)"; fi
    # An independent FAT checker on the volumes the tests wrote (target/fat/*.img, raw images: a recorded and saved
    # ride after a cut and its resume, a HISTORY delete, a full card's refusal, a create and move, a cut right after a
    # create): fsck_msdos on macOS, fsck.vfat on Linux, read only (-n). Any error it reports fails.
    # *_lost<N>.img: a cut the tests made on purpose that leaves a lost chain of N clusters (a course's, between the
    # remove and the link, or a delete's entry marked before its chain freed). Passes only on the exit code the tool
    # gives for orphan clusters alone (fsck_msdos 206; fsck.fat 1, inferred), every output line one of that tool's
    # (an allow-list, each pattern anchored; fsck.fat's inferred from dosfstools' messages, not seen here), and N
    # orphans counted. fsck_msdos does not compare the two FAT copies: the tests' own fsck does (fat/write.rs).
    fsck_lost() {
        want=$(echo "$1" | sed -E 's/.*_lost([0-9]+)\.img$/\1/')
        case "$fs" in
            fsck_msdos*) ok=206
                allow='^\*\* |^Warning: Found orphan cluster\(s\)$|^Fix\?$|^: no$|^Warning: Found [0-9]+ orphaned clusters$|^Warning: [0-9]+ files, .* free \([0-9]+ clusters\)$'
                got=$(sed -nE 's/^Warning: Found ([0-9]+) orphaned clusters$/\1/p' "$3");;
            *) ok=1
                allow='^fsck\.fat |^Reclaimed [0-9]+ unused clusters .* in 1 chain\.$|^Free cluster summary wrong .*$|^  Auto-correcting\.$|^Leaving filesystem unchanged\.$|: [0-9]+ files, [0-9]+/[0-9]+ clusters$'
                got=$(sed -nE 's/^Reclaimed ([0-9]+) unused clusters .*/\1/p' "$3");;
        esac
        [ "$2" = "$ok" ] && [ "$got" = "$want" ] && ! grep -vE "$allow" "$3" | grep -q .
    }
    fat_check() {
        PATH="$PATH:/sbin:/usr/sbin"
        if command -v fsck_msdos >/dev/null 2>&1; then fs="fsck_msdos -n"
        elif command -v fsck.vfat >/dev/null 2>&1; then fs="fsck.vfat -n"
        elif [ $strict = 1 ]; then echo "fsck: no fsck_msdos / fsck.vfat: on main (or --full) an independent FAT checker is required"; return 1
        else skip "fsck (no fsck_msdos / fsck.vfat)"; return 0; fi
        n=0
        for img in target/fat/*.img; do
            [ -f "$img" ] || continue
            n=$((n + 1))
            if $fs "$img" > "$C/fsck.out" 2>&1; then continue; else code=$?; fi
            case "$img" in *_lost[0-9]*.img) fsck_lost "$img" "$code" "$C/fsck.out" && continue;; esac
            cat "$C/fsck.out"; echo "fsck: $fs failed on $img"; return 1
        done
        [ $n -ge 5 ] || { echo "fsck: only $n images in target/fat (5 expected: the tests write them)"; return 1; }
        echo "fsck: $n images clean"
    }
    step "fat fsck" fat_check
    # What the tests wrote (target/fit, target/sim): the test rides decoded by independent readers (fitdecode,
    # Garmin's FIT SDK; FIT_PYTHON may name a venv's Python), and the unit walk's path filter (needs PIL; plain
    # python3, not -I, which drops the user site-packages where PIL may be).
    PY="${FIT_PYTHON:-python3}"
    if "$PY" -c 'import fitdecode, garmin_fit_sdk' 2>/dev/null; then
        step "fit_check" sh -e -c '
            "$0" tools/fit_check.py target/fit/sample.fit target/fit/cut.fit target/fit/sample_full.fit
            "$0" tools/fit_check.py --pair target/fit/recorded.fit target/fit/recorded_full.fit
            "$0" tools/fit_check.py --profile target/fit/indoor.fit target/fit/older.fit target/fit/mtb.fit
            "$0" tools/fit_check.py --timer target/fit/autopause.fit target/fit/autoresume*.fit' "$PY"
    elif [ $strict = 1 ]; then
        echo "fit_check: fitdecode / garmin-fit-sdk are not installed: on main (or --full) the independent FIT readers are required"
        return 1
    else
        skip "fit_check (fitdecode / garmin-fit-sdk not installed)"
    fi
    if ! python3 -c 'import PIL' 2>/dev/null; then
        skip "walk (PIL)"
    elif ! touched '^(tools/|test-data/|renode/|docs/theming\.md|src/sim/walk_tests\.rs|check\.sh)'; then
        skip "walk selftest (nothing under tools/ changed)"
    else
        step "walk selftest" python3 tools/walk.py selftest
    fi
}

lane_P() {  # the Python tools' self-tests
    if ! touched '^(tools/|test-data/|renode/|docs/theming\.md|src/ui/tiles\.rs|check\.sh)'; then
        skip "tool self-tests (nothing under tools/, test-data/, renode/, docs/theming.md changed)"
    else
        step "lint.py selftest" python3 -I tools/lint.py selftest
        step "numbers.py selftest" python3 -I tools/numbers.py selftest
        step "dev.py selftest" python3 -I tools/dev.py selftest
        step "sync.py selftest" python3 -I tools/sync.py selftest  # a fake Intervals.icu and Strava on 127.0.0.1
        step "assist.py selftest" python3 -I tools/assist.py selftest
        step "segments.py selftest" python3 -I tools/segments.py selftest
        step "s810_probe.py selftest" python3 -I tools/s810_probe.py selftest
        step "route_pack.py selftest" python3 -I tools/route_pack.py selftest
        step "ign_pack.py selftest" python3 -I tools/ign_pack.py selftest
        step "test_select.py selftest" python3 -I tools/test_select.py selftest
        step "review.py selftest" python3 -I tools/review.py selftest
        step "map_pack.py selftest" python3 -I tools/map_pack.py selftest
        step "theme.py selftest" python3 -I tools/theme.py selftest
        step "iaprecover.py selftest" python3 -I tools/iaprecover.py selftest  # fakes only, no USB
        step "nrf_emu.py selftest" "${NRF_PYTHON:-python3}" -I tools/nrf_emu.py selftest  # NRF_PYTHON: a Python with unicorn; skips without the binary or unicorn
        step "fetch_firmware.py selftest" python3 -I tools/fetch_firmware.py selftest  # a fake download
    fi
    # src/ui/fonts.rs is generated by tools/font.py: regenerate into a temporary file and compare.
    if ! python3 -c 'import PIL' 2>/dev/null; then
        skip "font (PIL)"
    elif ! touched '^(tools/font\.py|tools/pixel_font\.py|tools/fonts/|src/ui/fonts\.rs)'; then
        skip "font regeneration (no change in tools/font.py, pixel_font.py, fonts/, src/ui/fonts.rs)"
    else
        fontcheck() {
            fontdir=$(mktemp -d)
            python3 tools/font.py -o "$fontdir/fonts.rs" >/dev/null || { rm -rf "$fontdir"; echo "tools/font.py failed"; return 1; }
            cmp -s "$fontdir/fonts.rs" src/ui/fonts.rs || { rm -rf "$fontdir"; echo "src/ui/fonts.rs differs from tools/font.py's output: run python3 tools/font.py"; return 1; }
            rm -rf "$fontdir"
        }
        step "font regeneration" fontcheck
    fi
}

( lane_F ) > "$C/F.log" 2>&1 & pF=$!
( lane_H ) > "$C/H.log" 2>&1 & pH=$!
( lane_P ) > "$C/P.log" 2>&1 & pP=$!
trap 'kill 0' INT TERM   # Ctrl-C stops the lane subshells too
bad=""
wait "$pF" || bad="$bad F"
wait "$pH" || bad="$bad H"
wait "$pP" || bad="$bad P"
for l in F H P; do
    case " $bad " in *" $l "*) show=1 ;; *) show=$verbose ;; esac
    if [ "$show" = 1 ]; then echo "=== lane $l ==="; cat "$C/$l.log"; fi
done
if [ -n "$bad" ]; then echo "check.sh: FAILED lane(s):$bad (logs in $C/)"; exit 1; fi

# The firmware on Renode's STM32H747 (renode/run.py), when installed (docs/devloop.md, Emulation). After the lanes and
# only when they passed: it compares the boot screen with target/sim/preview.bmp (lane H's test-host) and needs the
# image (lane F).
export RENODE="${RENODE:-/Applications/Renode.app/Contents/MacOS/renode}"
if ! command -v "$RENODE" >/dev/null 2>&1; then
    skip "renode (not installed)"
elif ! touched '^(src/main\.rs|src/lib\.rs|src/app/pass\.rs|src/board/|src/drivers/|src/ui/strip\.rs|src/ui/overview\.rs|link\.x|build\.rs|Cargo\.(toml|lock)|\.cargo/config\.toml|rust-toolchain\.toml|renode/)'; then
    skip "renode (no change in main.rs, lib.rs, app/pass.rs, board/, drivers/, ui/strip.rs, ui/overview.rs, link.x, build.rs, Cargo.toml/lock, .cargo/config.toml, rust-toolchain.toml, renode/)"
else
    renode2() {  # the boot (first screen, then the cable in) and the switch-off; in sequence unless RENODE_PARALLEL=1
        if [ "${RENODE_PARALLEL:-0}" = 1 ]; then
            python3 renode/run.py > "$C/renode-boot.log" 2>&1 & p1=$!
            python3 renode/run.py --off > "$C/renode-off.log" 2>&1 & p2=$!
            r=0; wait $p1 || r=1; wait $p2 || r=1
        else
            r=0
            python3 renode/run.py > "$C/renode-boot.log" 2>&1 || r=1
            python3 renode/run.py --off > "$C/renode-off.log" 2>&1 || r=1
        fi
        cat "$C"/renode-*.log
        return $r
    }
    step "renode (boot + vbus, off)" renode2 || { echo "check.sh: FAILED renode"; exit 1; }
fi

# The live numbers of the docs (markers, AGENTS.md Docs) need the ELF (lane F) and the test list (lane H). They are
# written on main (or --full), by the merge step; a branch only reports the stale ones, so it never commits marker
# churn that conflicts on merge.
if [ $full = 1 ] || [ "$br" = main ]; then
    step "numbers.py --write" python3 -I tools/numbers.py --write
else
    echo "numbers (information only, written on main):"
    step "numbers.py --check" sh -c 'python3 -I tools/numbers.py --check || true'
fi

t_end=$(now)
echo "--- timings (the three lanes ran in parallel) ---"
cat "$C/times"
awk -v a="$t_all" -v b="$t_end" 'BEGIN { printf "[%6.1f s] TOTAL\n", b - a }'
if [ -s "$C/skipped" ]; then echo "SKIPPED: $(paste -s -d ';' "$C/skipped" | sed 's/;/; /g')"; fi
