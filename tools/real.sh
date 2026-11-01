#!/bin/sh
# The #[ignore] tests that read the owner's real data (rides, drive, tiles, routes, segments), before a ride build.
# Each panics with its variable's name when it is unset, so a run that proves nothing cannot pass. The variables
# come from tools/real.env (gitignored, never committed: paths and data stay on this computer), one `export NAME=...`
# a line: BRYTON_RIDES, BRYTON_DRIVE, BRYTON_TILES, BRYTON_ROUTE_DAT, BRYTON_PLANTRIP, IGN_PACKS, HRVT_FILES,
# SEGMENTS_DIR. theme_preview is not real data (tools/theme.py runs it with THEME_FILE).
set -e
cd "$(dirname "$0")/.."
[ -f tools/real.env ] || { echo "real.sh: tools/real.env is missing (export NAME=path lines, see this script)"; exit 1; }
. ./tools/real.env
exec cargo test-host -- --ignored --skip theme_preview "$@"
