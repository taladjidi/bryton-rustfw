#!/usr/bin/env python3
"""Which host tests a branch's changed paths can affect (check.sh --quick): a small, conservative table.

  git diff --name-only BASE | test_select.py     prints ALL, NONE, or the libtest name filters, one per line
  test_select.py selftest                        the table on a few diffs, and on every file the tests read (check.sh)

Anything unmapped or shared gives ALL: src/lib.rs, src/sim/mod.rs, sim/tests.rs and any src/sim file that is not a
`*_tests.rs` or fuzz.rs (the simulator's models), src/storage (its FAT image helpers are used by tests everywhere),
Cargo.*, check.sh, test-data/, a new folder, and any doc a test reads (found by the selftest in src/). Only Markdown
in docs/, tools/ and renode/ give NONE. The full gate never uses this: it always runs every test.
"""
import re
import sys
from pathlib import Path

# top folder of src/ -> the unit tests of the folder, and the simulator and app tests that run its code
TABLE = {
    'nav': ['nav::', 'sim::', 'app::'],
    'training': ['training::', 'sim::', 'app::'],
    'link': ['link::', 'sim::', 'app::'],
    'ui': ['ui::', 'sim::', 'app::'],
    'drivers': ['drivers::', 'sim::', 'app::'],
    'app': ['app::', 'sim::'],
}


def select(paths):
    """The filters for these changed paths, or ['ALL'] / ['NONE']."""
    out = set()
    read = files_the_tests_read(Path(__file__).resolve().parent.parent)
    for p in paths:
        if not p:
            continue
        if p in read:  # a file a test reads (a doc included): every test may depend on it
            return ['ALL']
        if re.match(r'(tools/|renode/)', p) or re.match(r'docs/[^/]*\.md$', p) and p != 'docs/features.md' or p.endswith('README.md'):
            continue
        m = re.match(r'src/([a-z_]+)/', p)
        s = re.match(r'src/sim/([a-z_0-9]+)\.rs$', p)
        if m and m[1] in TABLE:
            out.update(TABLE[m[1]])
        elif s and (s[1].endswith('_tests') and s[1] != 'tests' or s[1] == 'fuzz'):
            out.add('sim::' + s[1])
        else:
            return ['ALL']
    return sorted(out) or ['NONE']


def files_the_tests_read(root):
    """Paths under docs/, test-data/, tools/ that a source file names (on a line with include_str!, include_bytes!, fs::read or CARGO_MANIFEST_DIR)."""
    found = set()
    for f in Path(root, 'src').rglob('*.rs'):
        for line in f.read_text().splitlines():
            if re.search(r'include_str!|include_bytes!|fs::read|CARGO_MANIFEST_DIR', line):
                found.update(m[1] for m in re.finditer(r'((?:docs|test-data|tools)/[A-Za-z0-9_./-]+)', line))
    return found


def selftest():
    assert select(['docs/a.md', 'tools/x.py']) == ['NONE']
    assert select([]) == ['NONE']
    for p in ['src/lib.rs', 'src/sim/mod.rs', 'src/sim/tests.rs', 'src/sim/ble_sim.rs', 'src/sim/loop_model.rs',
              'src/storage/fit.rs', 'src/main.rs', 'src/newdir/x.rs', 'Cargo.lock', 'check.sh', 'test-data/walk.hashes',
              'docs/features.md', 'link.x']:
        assert select([p]) == ['ALL'], p
    assert select(['Cargo.lock', 'src/ui/text.rs']) == ['ALL']
    assert select(['src/sim/course_tests.rs']) == ['sim::course_tests']
    assert select(['src/sim/fuzz.rs']) == ['sim::fuzz']
    assert select(['src/ui/text.rs', 'docs/x.md']) == ['app::', 'sim::', 'ui::']
    assert select(['src/nav/segment.rs']) == ['app::', 'nav::', 'sim::']
    # a file a test reads must never select NONE
    root = Path(__file__).resolve().parent.parent
    read = [p for p in files_the_tests_read(root) if (root / p).is_file() or (root / p.rstrip('/')).is_dir()]
    assert read, 'no file read by a test found: the scan is broken'
    for p in read:
        assert select([p]) != ['NONE'], f'{p} is read by a test but selects no test'
    print('test_select selftest ok')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
    else:
        print('\n'.join(select(sys.stdin.read().split('\n'))))
