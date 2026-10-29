#!/usr/bin/env python3
"""The firmware's stack frames against their budgets and the stack (check.sh, after the build).

The stack is DTCM from the canary above .bss to the top (link.x `_stack_canary` to `_stack_top`): on 2026-10-24
main's frame had grown to 111 KB (the app held there several times over) and opening a workout overflowed it into
.bss. The canary (main.rs `stack_check`) turns an overflow that reaches it into a reset at the end of the pass, but
cannot stop it: these budgets are the line. This reads each function's frame from its `push` and `sub sp, #n` in the
release ELF (llvm-objdump from the toolchain) and the stack's bounds (llvm-nm), and fails when a frame is over its
budget, or when main's frame and the largest other one do not fit the stack together (a lower bound of the boot's
depth: main calls make_app).

    stack_check.py            check the release ELF
    stack_check.py selftest   offline checks of the parsing (check.sh runs it)
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
# frames allowed, bytes: the app's start (one temporary copy of App, once at boot), the course memory's (part of it
# built on the stack, once at boot, after the app's; 41,000 since pushes count: 39,956 B on 2026-10-29), main,
# anything else
BUDGET = {'make_app': 64_000, 'make_courses': 41_000, 'main': 16_384}
OTHER = 20_480


def regs(names):
    """Registers in a push list: `r4, r5, lr` is 3, `r4-r11, lr` is 9."""
    n = 0
    for r in names.split(','):
        ends = [int(x) for x in re.findall(r'\d+', r)]
        n += ends[1] - ends[0] + 1 if '-' in r and len(ends) == 2 else 1
    return n


def frames(text):
    """Each function's frame in bytes from llvm-objdump text: its largest `sub sp`, push (4 B a core register),
    vpush (8 B a d register) and pre-indexed `str ..., [sp, #-n]!`, added (an overestimate when a function has
    several on different paths)."""
    big, fn = {}, None
    for line in text.splitlines():
        m = re.match(r'^[0-9a-f]+ <(.*)>:$', line)
        if m:
            fn = m.group(1)
            continue
        if not fn:
            continue
        for kind, pattern, size in (
                ('sub', r'\bsub(?:\.w|w)?\s+sp,\s*(?:sp,\s*)?#(0x[0-9a-f]+|\d+)', lambda g: int(g, 0)),
                ('push', r'\bpush(?:\.w)?\s+\{([^}]*)\}', lambda g: 4 * regs(g)),
                ('vpush', r'\bvpush\s+\{([^}]*)\}', lambda g: 8 * regs(g)),
                ('str', r'\bstr(?:\.w)?\s+\w+,\s*\[sp,\s*#-(\d+)\]!', int)):
            m = re.search(pattern, line)
            if m:
                k = (fn, kind)
                big[k] = max(big.get(k, 0), size(m.group(1)))
    out = {}
    for (f, _), s in big.items():
        out[f] = out.get(f, 0) + s
    return out


def symbols(text):
    """The stack's bounds from llvm-nm text."""
    return {m.group(2): int(m.group(1), 16) for m in re.finditer(r'^([0-9a-f]+) \w (\w+)$', text, re.M)}


def budget(f):
    return next((b for k, b in BUDGET.items() if re.search(rf'\b{k}\b|\d{k}\d|{k}17h', f)), OTHER)


def check(fr, syms):
    """The failures, a line each, and the summary line."""
    stack = syms['_stack_top'] - syms['_stack_canary'] - 4
    over = [f'{s} B > {budget(f)} B: {f}' for f, s in fr.items() if s > budget(f)]
    main = next((s for f, s in fr.items() if re.search(r'\d4main17h', f)), 0)
    largest = max((s for f, s in fr.items() if not re.search(r'\d4main17h', f)), default=0)
    if main + largest > stack:
        over.append(f'main {main} B + the largest other frame {largest} B > the stack {stack} B')
    top = sorted(fr.items(), key=lambda x: -x[1])[:3]
    summary = (f'stack {stack} B (canary at {syms["_stack_canary"]:#x}); frames: ' + ', '.join(f'{re.sub(r"17h[0-9a-f]+E$", "", f)[-40:]} {s}' for f, s in top))
    return over, summary


def selftest():
    text = '\n'.join([
        'd0000000 <default_handler>:', 'd0000000:      \tpush\t{r7, lr}',
        'd0000010 <_ZN13bryton_rustfw4main17h0123456789abcdefE>:', 'd0000010:      \tpush\t{r4, r5, r6, r7, lr}',
        'd0000012:      \tstr\tr11, [sp, #-4]!', 'd0000016:      \tvpush\t{d8, d9}', 'd0000018:      \tsub.w\tsp, sp, #0x1000',
        'd0000020 <_ZN13bryton_rustfw8make_app17h0123456789abcdefE>:', 'd0000020:      \tpush.w\t{r4-r11, lr}',
        'd0000024:      \tsub\tsp, #0x100'])
    fr = frames(text)
    assert fr == {'default_handler': 8, '_ZN13bryton_rustfw4main17h0123456789abcdefE': 20 + 4 + 16 + 0x1000,
                  '_ZN13bryton_rustfw8make_app17h0123456789abcdefE': 36 + 0x100}, fr
    syms = symbols('200011bc B _stack_canary\n20020000 A _stack_top\n')
    assert syms == {'_stack_canary': 0x200011bc, '_stack_top': 0x20020000}, syms
    assert check(fr, syms)[0] == [], check(fr, syms)
    small = dict(syms, _stack_top=0x200011c0 + 4096)
    assert any('the stack' in o for o in check(fr, small)[0]), check(fr, small)
    assert any('64000' in o for o in check(dict(fr, _ZN13bryton_rustfw8make_app17h0123456789abcdefE=70_000), syms)[0])
    print('stack_check.py selftest ok')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
        sys.exit()
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True, check=True).stdout.strip()
    bins = next(pathlib.Path(sysroot).glob('lib/rustlib/*/bin/llvm-objdump')).parent
    run = lambda tool, *a: subprocess.run([str(bins / tool), *a, str(ELF)], capture_output=True, text=True).stdout
    over, summary = check(frames(run('llvm-objdump', '-d', '--no-show-raw-insn')), symbols(run('llvm-nm')))
    print(summary)
    if over:
        sys.exit('stack over budget:\n' + '\n'.join(over))
