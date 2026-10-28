#!/usr/bin/env python3
"""The live numbers of the docs, computed from the build: a Markdown marker is never typed by hand.

  numbers.py                  the numbers, one `name value` line each
  numbers.py --write          rewrite every marker in the docs, printing each one it changed (check.sh)
  numbers.py --check          exit 1, listing the markers whose text is stale
  numbers.py selftest         the marker rewrite on in-memory text (check.sh)

A marker is `<!-- n:NAME -->TEXT<!-- /n -->`; only TEXT changes, the text around it is the writer's. The values:
  image        bytes of the image pack.py packs (its "image N B used"), from the release ELF
  axi_free     AXI SRAM bytes left: the start of PANELDMA (link.x) minus _eaxi, the end of the app's cells
  app, courses, ahead, scan_cache
               the sizes of the static cells in AXI SRAM (main.rs make_app, make_courses, make_ahead,
               make_scan_cache), the symbols' sizes in the ELF
  tests, tests_ignored, doctests
               the host tests of `cargo test-host` (unit and simulator tests, ignored ones included in `tests`),
               the ignored ones, and the doc tests, listed by libtest (no test is run here)
  rust_lines   lines of src/**/*.rs
KB is 1000 bytes, as the README has always counted it. Build first: `cargo build --release` (the ELF), then the
tests are listed from the same test profile as `cargo test-host`, so nothing is rebuilt for another profile.
Standard library only: the ELF's symbol table is read here, so no nm is needed on a clean machine.
"""
import argparse, glob, os, re, struct, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
import pack  # the image is pack.py's own computation (load_segments, BASE, ELF)

MARK = re.compile(r'<!-- n:([a-z_]+) -->([^\n]*?)<!-- /n -->')
# the static cells in AXI SRAM: the mangled prefix of each cell's symbol (link.x .axibss names the same)
CELLS = {
    'app': '_ZN13bryton_rustfw8make_app3APP',
    'courses': '_ZN13bryton_rustfw12make_courses7COURSES',
    'ahead': '_ZN13bryton_rustfw10make_ahead5AHEAD',
    'scan_cache': '_ZN13bryton_rustfw15make_scan_cache5CACHE',
}


def fmt_bytes(n):
    """625 KB: 1000-based, rounded.

    >>> fmt_bytes(624892)
    '625 KB'
    """
    return f'{round(n / 1000)} KB'


def fmt_count(n):
    """825, 1,234.

    >>> fmt_count(1234)
    '1,234'
    """
    return f'{n:,}'


def fmt_lines(n):
    """Rounded to the thousand, as the docs say "about 84,000".

    >>> fmt_lines(84123)
    '84,000'
    """
    return f'{round(n / 1000) * 1000:,}'


FORMAT = {'image': fmt_bytes, 'axi_free': fmt_bytes, 'app': fmt_bytes, 'courses': fmt_bytes,
          'ahead': fmt_bytes, 'scan_cache': fmt_bytes, 'tests': fmt_count, 'tests_ignored': fmt_count,
          'doctests': fmt_count, 'rust_lines': fmt_lines}


def elf_symbols(path):
    """{name: (value, size)} of the ELF32 little-endian symbol table (first definition of each name)."""
    e = open(path, 'rb').read()
    if e[:4] != b'\x7fELF' or e[4] != 1 or e[5] != 1:
        sys.exit(f'numbers.py: {path} is not a 32-bit little-endian ELF')
    shoff, = struct.unpack_from('<I', e, 0x20)
    shentsize, shnum = struct.unpack_from('<HH', e, 0x2e)
    secs = [struct.unpack_from('<10I', e, shoff + i * shentsize) for i in range(shnum)]  # Elf32_Shdr
    syms = {}
    for _name, typ, _flags, _addr, off, size, link, _info, _align, _esize in secs:
        if typ != 2:  # SHT_SYMTAB
            continue
        stroff = secs[link][4]
        for k in range(size // 16):  # Elf32_Sym
            st_name, value, sz, _info, _other, _shndx = struct.unpack_from('<IIIBBH', e, off + k * 16)
            start = stroff + st_name
            name = e[start:e.index(b'\0', start)].decode('ascii', 'replace')
            if name:
                syms.setdefault(name, (value, sz))
    return syms


def cargo_list(*args):
    """The test names libtest lists for `cargo test-host` with these args (stdout only)."""
    out = subprocess.run(['cargo', 'test-host', *args], cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f'numbers.py: cargo test-host {" ".join(args)} failed:\n{out.stderr[-2000:]}')
    return [ln for ln in out.stdout.splitlines() if ln.endswith(': test')]


def values():
    """The numbers, by name. A cell whose symbol is not in the ELF is left out, and said so on stderr."""
    elf = os.path.join(ROOT, 'target', 'thumbv7em-none-eabihf', 'release', 'bryton-rustfw')
    if not os.path.exists(elf):
        sys.exit('numbers.py: no release ELF: run cargo build --release first')
    v = {}
    v['image'] = max(p - pack.BASE + len(d) for p, d in pack.load_segments(elf))
    syms = elf_symbols(elf)
    v['axi_free'] = syms['_spaneldma'][0] - syms['_eaxi'][0]  # link.x: the panel DMA's buffers end the AXI SRAM
    for name, prefix in CELLS.items():
        hits = [sz for sym, (_val, sz) in syms.items() if sym.startswith(prefix)]
        if hits:
            v[name] = max(hits)
        else:
            print(f'numbers.py: no symbol {prefix}* in the ELF, {name} skipped', file=sys.stderr)
    v['tests'] = len(cargo_list('--lib', '--', '--list'))
    v['tests_ignored'] = len(cargo_list('--lib', '--', '--ignored', '--list'))
    v['doctests'] = len(cargo_list('--doc', '--', '--list'))
    v['rust_lines'] = sum(sum(1 for _ in open(f, encoding='utf-8')) for f in
                          glob.glob(os.path.join(ROOT, 'src', '**', '*.rs'), recursive=True))
    return v


def texts(vals):
    """The marker text of each value, as the docs show it."""
    return {name: FORMAT[name](n) for name, n in vals.items()}


def rewrite(text, marker_texts):
    """(text with every marker set to its value, [(name, old, new)] of the markers that changed).

    A marker name with no value is refused: a typo must not pass as a number.
    """
    changed = []

    def sub(m):
        name, old = m.group(1), m.group(2)
        if name not in marker_texts:
            raise ValueError(f'unknown marker n:{name}')
        new = marker_texts[name]
        if old != new:
            changed.append((name, old, new))
        return f'<!-- n:{name} -->{new}<!-- /n -->'

    return MARK.sub(sub, text), changed


def doc_files():
    return sorted(glob.glob(os.path.join(ROOT, '*.md')) + glob.glob(os.path.join(ROOT, 'docs', '*.md')))


def selftest():
    t = {'image': '625 KB', 'tests': '825', 'rust_lines': '84,000'}
    src = 'a <!-- n:image -->1 KB<!-- /n --> b\n<!-- n:tests -->1<!-- /n --> <!-- n:rust_lines -->84,000<!-- /n -->'
    new, ch = rewrite(src, t)
    assert new == 'a <!-- n:image -->625 KB<!-- /n --> b\n<!-- n:tests -->825<!-- /n --> <!-- n:rust_lines -->84,000<!-- /n -->', new
    assert [c[0] for c in ch] == ['image', 'tests'], ch  # the unchanged marker is not reported
    again, ch = rewrite(new, t)
    assert again == new and ch == [], 'round trip changed the text'
    try:
        rewrite('<!-- n:nope -->x<!-- /n -->', t)
        raise AssertionError('an unknown marker was accepted')
    except ValueError:
        pass
    assert fmt_bytes(624892) == '625 KB' and fmt_count(825) == '825' and fmt_lines(84123) == '84,000'
    print('numbers.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('cmd', nargs='?', choices=['selftest'])
    ap.add_argument('--write', action='store_true', help='rewrite the markers in the docs')
    ap.add_argument('--check', action='store_true', help='exit 1 when a marker is stale')
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    vals = values()
    if not (a.write or a.check):
        for name, n in vals.items():
            print(name, n)
        return
    mt = texts(vals)
    stale = []
    for path in doc_files():
        text = open(path, encoding='utf-8').read()
        try:
            new, changed = rewrite(text, mt)
        except ValueError as err:
            sys.exit(f'numbers.py: {os.path.relpath(path, ROOT)}: {err}')
        rel = os.path.relpath(path, ROOT)
        for name, old, now in changed:
            print(f'{rel}: n:{name} {old!r} -> {now!r}')
            stale.append(f'{rel}: n:{name}')
        if a.write and new != text:
            open(path, 'w', encoding='utf-8', newline='').write(new)
    if a.check and stale:
        print('numbers.py: stale markers (run tools/numbers.py --write):\n  ' + '\n  '.join(stale))
        sys.exit(1)


if __name__ == '__main__':
    main()
