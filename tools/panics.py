#!/usr/bin/env python3
"""Every panic site of the release firmware and the source line it comes from (an audit report, never a gate).

A panic marks the fault and resets the unit (hw.rs `default_handler`), so AGENTS.md "Panics" bans unwrap and indexing
that can fail on the firmware path. This reads the release ELF with `llvm-objdump -d -C -l` from the toolchain (which
ships no llvm-symbolizer): each call or branch to a panic entry (core::panicking, the slice index fails, unwrap /
expect, the panic handler) is a site. Its line is the `core::panic::Location` the call passes (a movw / movt pair
a few instructions before, pointing at a static in .rodata: file, line); with none, the line-table entry the objdump
prints before it (the innermost frame), or `?` when that is in core (no llvm-symbolizer is shipped to walk the
inlined frames).
Kinds: bounds, unwrap, overflow, explicit, trap (a `udf`). The summary lists the sites per folder and the sites in
files that deny clippy::indexing_slicing outside tests (there should be no bounds site). LTO can merge identical
panic calls into one site, so a line may stand for several.

    panics.py [ELF]            the report (default: the release ELF); exit 0 always
    panics.py [ELF] --json F   also write it as JSON to F
    panics.py selftest         offline checks of the parsing (check.sh may run it)
"""
import collections
import json
import pathlib
import re
import struct
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
# (substring of the demangled target, kind), first match wins
KINDS = [('panic_bounds_check', 'bounds'), ('core::slice::index::slice_', 'bounds'), ('slice_error_fail', 'bounds'),
         ('unwrap_failed', 'unwrap'), ('expect_failed', 'unwrap'),
         ('div_by_zero', 'overflow'), ('rem_by_zero', 'overflow'), ('_overflow', 'overflow'),
         ('core::panicking::', 'explicit'), ('rust_begin_unwind', 'explicit'), ('board::hw::panic', 'explicit')]
BRANCH = re.compile(r'b(?:lx?)?(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[wn])?')


def location(rd, recent):
    """'src/x.rs:LINE' from the first Location static that a movw / movt pair in `recent` (reg -> [lo, hi]) points to;
    rd(addr, n) reads the image. A Location is {file ptr, file len, line, column}, 4 bytes each."""
    for lo, hi in recent.values():
        if lo is not None and hi is not None:
            ptr, n, line, _ = struct.unpack('<4I', rd((hi << 16) | lo, 16) or bytes(16))
            m = re.fullmatch(rb'(src/[\w/.-]+\.rs)', (rd(ptr, n) or b'') if n < 200 else b'')
            if m:
                return f'{m.group(1).decode()}:{line}'
    return None


def sites(text, rd=lambda a, n: None):
    """(kind, enclosing function, 'src/x.rs:LINE' or '?') of each panic call or branch, and `udf` traps, in
    llvm-objdump -d -C -l text (a `; /path/src/x.rs:LINE` line gives the line of the instructions below it); rd reads
    the image for the Location (see `location`)."""
    out, fn, loc, recent = [], '', '?', {}
    for line in text.splitlines():
        if line.startswith('; '):
            m = re.match(rf'^; {re.escape(str(ROOT))}/(src/[^:]+):(\d+)', line)
            loc = f'{m.group(1)}:{m.group(2)}' if m else '?'
            continue
        m = re.match(r'^[0-9a-f]+ <(.*)>:$', line)
        if m:
            fn, recent = m.group(1), {}
            continue
        m = re.match(r'^\s*[0-9a-f]+:\s+(\w[\w.]*)\s*(.*)$', line)
        if not m:
            continue
        op, rest = m.group(1), m.group(2)
        w = re.match(r'mov([wt])\s+(\w+), #(0x[0-9a-f]+|\d+)', f'{op} {rest}')
        if w:
            recent.setdefault(w.group(2), [None, None])[w.group(1) == 't'] = int(w.group(3), 0)
        if op.startswith('udf'):
            out.append(('trap', fn, loc))
            continue
        if len(recent) > 6:
            recent = dict(list(recent.items())[-6:])
        t = re.search(r'<([^>]*)>', rest)
        if t and BRANCH.fullmatch(op):
            kind = next((k for s, k in KINDS if s in t.group(1)), None)
            if kind:
                out.append((kind, fn, location(rd, recent) or loc))
                recent = {}
    return out


def image(path):
    """rd(addr, n): n bytes at a virtual address of the ELF's PT_LOAD segments, or None."""
    d = path.read_bytes()
    phoff, = struct.unpack_from('<I', d, 28)
    size, num = struct.unpack_from('<HH', d, 42)
    segs = [struct.unpack_from('<8I', d, phoff + i * size) for i in range(num)]

    def rd(a, n):
        for t, off, va, _, fsz, _, _, _ in segs:
            if t == 1 and va <= a and a + n <= va + fsz:
                return d[off + a - va:off + a - va + n]
        return None
    return rd


def denied():
    """Files (and for a mod.rs its whole folder) that deny clippy::indexing_slicing outside tests."""
    out = []
    for f in sorted((ROOT / 'src').rglob('*.rs')):
        if re.search(r'deny\(clippy::indexing_slicing', f.read_text()):
            out.append(str(f.parent.relative_to(ROOT)) + '/' if f.name == 'mod.rs' else str(f.relative_to(ROOT)))
    return out


def report(found, deny):
    """Text report: by file, every site, the summary; and the same as a dict."""
    rows = sorted((loc.rsplit(':', 1)[0], int(loc.rsplit(':', 1)[1]) if ':' in loc else 0, k, fn) for k, fn, loc in found)
    by = collections.defaultdict(collections.Counter)
    for f, _, k, _ in rows:
        by[f][k] += 1
    out = ['count  kinds  file']
    for f, c in sorted(by.items(), key=lambda x: (-sum(x[1].values()), x[0])):
        out.append(f'{sum(c.values()):5}  ' + ' '.join(f'{k}={n}' for k, n in sorted(c.items())) + f'  {f}')
    out.append('')
    out += [f'{f}:{n}  {k}  in {fn}' for f, n, k, fn in rows]
    folders = collections.Counter(re.sub(r'^(src/\w+)/.*', r'\1', f) for f, *_ in rows)
    out += ['', f'total sites: {len(rows)}'] + [f'  {f}: {n}' for f, n in sorted(folders.items())]
    bad = [(f, n, k, fn) for f, n, k, fn in rows if k == 'bounds' and any(f.startswith(d) for d in deny)]
    out.append(f'bounds sites in indexing_slicing-denied files: {len(bad)}')
    out += [f'  {f}:{n} in {fn}' for f, n, _, fn in bad]
    return '\n'.join(out), {'total': len(rows), 'files': {f: dict(c) for f, c in by.items()}, 'folders': dict(folders),
                            'sites': [dict(file=f, line=n, kind=k, function=fn) for f, n, k, fn in rows],
                            'denied_bounds': [dict(file=f, line=n) for f, n, *_ in bad]}


def selftest():
    text = '\n'.join([
        'd0000000 <app::step>:', f'; {ROOT}/src/app/a.rs:42', 'd0000000: \tmovw\tr2, #0x100', 'd0000002: \tmovt\tr2, #0xd000',
        'd0000000: \tbl\t0xd0009000 <core::panicking::panic_bounds_check::h7761f4e897d698b2> @ imm = #0x9334c',
        '; /x/rustlib/src/rust/library/core/src/option.rs:9',
        'd0000004: \tbhi.w\t0xd0009010 <core::option::unwrap_failed::h01>', f'; {ROOT}/src/storage/fit.rs:9',
        'd0000008: \tbl\t0xd0009020 <core::panicking::panic_const::panic_const_add_overflow>',
        'd000000c: \tbl\t0xd0009030 <core::fmt::write>', 'd0000010: \tbic\tr0, r1', 'd0000012: \tudf\t#254',
        'd0000014: \tb\t0xd0009040 <core::slice::index::slice_end_index_len_fail>'])
    mem = {0xd0000100: struct.pack('<4I', 0xd0000200, 11, 77, 5), 0xd0000200: b'src/ui/t.rs'}
    got = sites(text, lambda a, n: mem.get(a))
    assert got == [('bounds', 'app::step', 'src/ui/t.rs:77'), ('unwrap', 'app::step', '?'),
                   ('overflow', 'app::step', 'src/storage/fit.rs:9'), ('trap', 'app::step', 'src/storage/fit.rs:9'),
                   ('bounds', 'app::step', 'src/storage/fit.rs:9')], got
    txt, js = report(got[:2], ['src/storage/fit.rs'])
    assert js['total'] == 2 and js['denied_bounds'] == [] and 'bounds=1' in txt, txt
    txt, js = report(got[3:], ['src/storage/'])
    assert js['denied_bounds'] == [{'file': 'src/storage/fit.rs', 'line': 9}], js
    print('panics.py selftest ok')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
        sys.exit()
    args = sys.argv[1:]
    jf = args.pop(args.index('--json') + 1) if '--json' in args else None
    elf = [a for a in args if a != '--json']
    elf = pathlib.Path(elf[0]) if elf else ELF
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True, check=True).stdout.strip()
    bins = next(pathlib.Path(sysroot).glob('lib/rustlib/*/bin/llvm-objdump')).parent
    dis = subprocess.run([str(bins / 'llvm-objdump'), '-d', '-C', '-l', '--no-show-raw-insn', str(elf)],
                         capture_output=True, text=True).stdout
    txt, js = report(sites(dis, image(elf)), denied())
    print(txt)
    if jf:
        pathlib.Path(jf).write_text(json.dumps(js, indent=1))
