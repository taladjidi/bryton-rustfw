#!/usr/bin/env python3
"""Which host tests a branch's changed paths can affect (check.sh --quick): a small conservative table.

  git diff --name-only BASE | test_select.py     prints ALL, NONE, or the libtest name filters, one per line
  test_select.py selftest                        the table on a few diffs (check.sh)

Anything unmapped or shared (src/lib.rs, src/sim/mod.rs, Cargo.*, check.sh, a new file: the caller lists new paths
too, they are just paths here) gives ALL. Only docs, tools, renode, test-data and Markdown give NONE: no Rust
changed. The full gate never uses this: it always runs every test.
"""
import re
import sys

# top folder of src/ -> the unit tests of the folder, and the simulator tests that run its code
TABLE = {
    'storage': ['storage::', 'sim::recorder_tests', 'sim::drive_tests', 'sim::storage_tests', 'sim::emmc_tests',
                'sim::stock_tests', 'sim::diag_tests', 'sim::ride_tests', 'sim::fuzz'],
    'nav': ['nav::', 'sim::course_tests', 'sim::altitude_tests', 'sim::segment_tests', 'sim::poi_tests',
            'sim::map_tests', 'sim::places_tests', 'sim::start_cue_tests', 'sim::fuzz'],
    'training': ['training::', 'sim::power_tests', 'sim::curve_tests', 'sim::ramp_tests', 'sim::pmc_tests',
                 'sim::workout_tests', 'sim::trainer_tests', 'sim::ftms_tests', 'sim::fecble_tests',
                 'sim::powercal_tests', 'sim::perf_tests', 'sim::fuzz'],
    'link': ['link::', 'sim::dev_tests', 'sim::link_tests', 'sim::phone_tests', 'sim::drive_tests', 'sim::fuzz'],
}
# the whole simulator runs on these: their own unit tests, and every simulator test
SIM_WIDE = ['ui', 'drivers', 'app']


def select(paths):
    """The filters for these changed paths, or ['ALL'] / ['NONE']."""
    out = set()
    for p in paths:
        if not p:
            continue
        if re.match(r'(docs/|tools/|renode/|test-data/)|[^/]*\.md$', p):
            continue
        m = re.match(r'src/([a-z_]+)/', p)
        s = re.match(r'src/sim/([a-z_0-9]+)\.rs$', p)
        if m and m[1] in TABLE:
            out.update(TABLE[m[1]])
        elif m and m[1] in SIM_WIDE:
            out.update([m[1] + '::', 'sim::'])
        elif s and s[1] not in ('mod', 'tests'):
            out.add('sim::' + s[1])
        else:
            return ['ALL']
    return sorted(out) or ['NONE']


def selftest():
    assert select(['docs/a.md', 'tools/x.py']) == ['NONE']
    assert select([]) == ['NONE']
    assert select(['src/lib.rs']) == ['ALL']
    assert select(['src/sim/mod.rs']) == ['ALL']
    assert select(['Cargo.lock', 'src/ui/text.rs']) == ['ALL']
    assert select(['src/sim/course_tests.rs']) == ['sim::course_tests']
    assert select(['src/ui/text.rs', 'docs/x.md']) == ['sim::', 'ui::']
    assert 'storage::' in select(['src/storage/trash.rs']) and 'sim::recorder_tests' in select(['src/storage/fit.rs'])
    assert select(['src/storage/fat/write.rs', 'src/lib.rs']) == ['ALL']
    assert select(['src/newdir/x.rs']) == ['ALL']
    print('test_select selftest ok')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
    else:
        print('\n'.join(select(sys.stdin.read().split('\n'))))
