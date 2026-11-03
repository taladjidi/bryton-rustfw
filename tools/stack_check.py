#!/usr/bin/env python3
"""The firmware's stack frames against their budgets and the stack (check.sh, after the build).

The stack is DTCM from the canary above .bss to the top (link.x `_stack_canary` to `_stack_top`): on 2026-10-24
main's frame had grown to 111 KB (the app held there several times over) and opening a workout overflowed it into
.bss. The canary (lib app/main_loop.rs `stack_ok`) turns an overflow that reaches it into a reset at the end of the pass, but
cannot stop it: these budgets are the line. This reads each function's frame from its `push` and `sub sp, #n` in the
release ELF (llvm-objdump from the toolchain) and the stack's bounds (llvm-nm), and fails when a frame is over its
budget, or when main's frame and the largest other one do not fit the stack together (a lower bound of the boot's
depth: main calls make_app).

    stack_check.py [ELF]      check the release ELF (check.sh also runs it on the ride build's)
    stack_check.py selftest   offline checks of the parsing and the call chain (check.sh runs it)

The code in ITCM is checked too (`itcm_calls`): hw.rs `stop_itcm` runs while the SDRAM is in self-refresh (docs/power.md
"Stop mode"), so it must not call, branch or point anywhere outside ITCM (a `bl`, a branch out, a call through a
register, a literal word in the FMC's SDRAM space 0xc0000000..0xdfffffff would fetch from the sleeping SDRAM). The
RAM-run trampoline is exempt: it writes the image and jumps into it on purpose.

The frames are also summed along the longest call chain (`chain`: a depth-first search over the `bl` call graph; a
cycle, i.e. recursion, is cut where it closes and listed). It fails above the stack minus MARGIN. `blx rN` calls
(through a pointer: closures and trait objects) cannot be followed: their callers are listed, so a chain through
one is a lower bound. A call is counted as 4 B for its return address in the frame (`push {..., lr}` has it).
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
# frames allowed, bytes: the app's start (one temporary copy of App, once at boot), the course memory's (part of it
# built on the stack, once at boot, after the app's; 41,000 since pushes count: 39,956 B on 2026-10-29), main,
# anything else
BUDGET = {'make_app': 64_000, 'make_courses': 41_000, 'main': 16_384}
OTHER = 20_480
MARGIN = 8_192  # the stack the longest static chain must leave free


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


def calls(text):
    """(direct calls fn -> {callee}, functions with a call through a register: `blx rN`, `bx rN` (not lr), `ldr pc`) from llvm-objdump text; a `b`/`bl` to another function's
    first instruction (a tail call included) is a call, a branch inside the function is not; so is a conditional tail branch (`bne.w` to another symbol)."""
    out, indirect, fn = {}, set(), None
    for line in text.splitlines():
        m = re.match(r'^[0-9a-f]+ <(.*)>:$', line)
        if m:
            fn = m.group(1)
            out.setdefault(fn, set())
            continue
        if not fn:
            continue
        m = re.search(r'\b(?:bl|b(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[wn])?)\s+0x[0-9a-f]+ <([^>+]*)>', line)
        if m and m.group(1) != fn:
            out[fn].add(m.group(1))
        elif re.search(r'\bblx\s+r\d+|\bbx\s+(?!lr\b)r\d+|\bldr(?:\.w)?\s+pc,|\bmov\s+pc,', line):
            indirect.add(fn)
    return out, indirect


def chain(fr, graph):
    """(deepest chain's bytes, its functions, the cycles found) of the longest path over `graph` (fn -> callees),
    computed on the condensation into strongly connected components (Tarjan), so a cycle never poisons a cached
    result. A component's weight is the SUM of its members' frames: recursion has no static bound, and one pass through
    every member is the least that can be true, so it stays an honest lower bound without being tuned to an edge
    order. The cycles are the components of more than one function."""
    sys.setrecursionlimit(max(10_000, 4 * len(graph)))
    idx, low, on, stack, comp, comps = {}, {}, set(), [], {}, []

    def tarjan(v):
        idx[v] = low[v] = len(idx)
        stack.append(v)
        on.add(v)
        for w in sorted(graph.get(v, ())):
            if w not in idx:
                tarjan(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], idx[w])
        if low[v] == idx[v]:
            c = []
            while True:
                w = stack.pop()
                on.discard(w)
                comp[w] = len(comps)
                c.append(w)
                if w == v:
                    break
            comps.append(sorted(c))

    for v in sorted(graph):
        if v not in idx:
            tarjan(v)
    # Tarjan emits a component after all it reaches, so comps is in reverse topological order: callees first
    best = []
    for i, c in enumerate(comps):
        out = {comp[w] for v in c for w in graph.get(v, ()) if w in comp and comp[w] != i}
        sub = max((best[j] for j in out), key=lambda r: r[0], default=(0, []))
        best.append((sum(fr.get(v, 0) for v in c) + sub[0], [c[0]] + sub[1]))
    top = max(best, key=lambda r: r[0], default=(0, []))
    return top[0], top[1], [' -> '.join(c) for c in comps if len(c) > 1]


def itcm_calls(text, syms):
    """The ITCM functions' reaches outside ITCM, a line each, from llvm-objdump text (`ram_trampoline` exempt); and
    the failure if hw.rs `stop_itcm` is not there at all (its section name or link.x changed)."""
    lo, hi = syms.get('_sitcm', 0), syms.get('_eitcm', 0)
    out, fn, seen = [], None, False
    for line in text.splitlines():
        m = re.match(r'^([0-9a-f]+) <(.*)>:$', line)
        if m:
            inside = lo <= int(m.group(1), 16) < hi and 'ram_trampoline' not in m.group(2)
            fn = m.group(2) if inside else None
            seen |= inside and 'stop_itcm' in m.group(2)
            continue
        if fn is None or not line.strip():
            continue
        short = re.sub(r'17h[0-9a-f]+E$', '', fn)[-40:]
        b = re.search(r'\b(bl|blx|b(?:eq|ne|cs|hs|cc|lo|mi|pl|vs|vc|hi|ls|ge|lt|gt|le)?(?:\.[wn])?)\s+0x([0-9a-f]+)\b', line)
        word = re.search(r'\.word\s+0x([0-9a-f]+)', line)
        movt = re.search(r'\bmovt(?:\.w)?\s+\w+,\s*#(0x[0-9a-f]+|\d+)', line)
        if movt and 0xc000 <= int(movt.group(1), 0) < 0xe000:   # the high half of an SDRAM address built in code
            out.append(f'{re.sub(r"17h[0-9a-f]+E$", "", fn)[-40:]}: points into the SDRAM: {line.strip()}')
            continue
        if b and (b.group(1) in ('bl', 'blx') or not lo <= int(b.group(2), 16) < hi):
            out.append(f'{short}: leaves ITCM: {line.strip()}')
        elif re.search(r'\bblx\s+r\d+|\bbx\s+(?!lr\b)r\d+|\bldr(?:\.w)?\s+pc,|\bmov\s+pc,', line):
            out.append(f'{short}: calls through a register: {line.strip()}')
        elif word and 0xc000_0000 <= int(word.group(1), 16) < 0xe000_0000:
            out.append(f'{short}: points into the SDRAM: {line.strip()}')
    if not seen:
        out.append('hw.rs stop_itcm is not in ITCM (_sitcm.._eitcm)')
    return out


def symbols(text):
    """The stack's bounds from llvm-nm text."""
    return {m.group(2): int(m.group(1), 16) for m in re.finditer(r'^([0-9a-f]+) \w (\w+)$', text, re.M)}


def budget(f):
    return next((b for k, b in BUDGET.items() if re.search(rf'\b{k}\b|\d{k}\d|{k}17h', f)), OTHER)


def check(fr, syms, graph=None, indirect=()):
    """The failures, a line each, and the summary line."""
    stack = syms['_stack_top'] - syms['_stack_canary'] - 4
    over = [f'{s} B > {budget(f)} B: {f}' for f, s in fr.items() if s > budget(f)]
    main = next((s for f, s in fr.items() if re.search(r'\d4main17h', f)), 0)
    largest = max((s for f, s in fr.items() if not re.search(r'\d4main17h', f)), default=0)
    if main + largest > stack:
        over.append(f'main {main} B + the largest other frame {largest} B > the stack {stack} B')
    top = sorted(fr.items(), key=lambda x: -x[1])[:3]
    extra = ''
    if graph is not None:
        deep, path, cycles = chain(fr, graph)
        short = lambda f: re.sub(r'17h[0-9a-f]+E$', '', f)[-34:]
        if deep > stack - MARGIN:
            over.append(f'the longest call chain {deep} B > the stack {stack} B - {MARGIN} B: ' + ' -> '.join(short(f) for f in path[:12]))
        extra = f'; longest chain {deep} B in {len(path)} calls, {len(cycles)} cycles cut, {len(indirect)} functions call through a pointer'
        extra += ''.join(f'\n  cycle: {" -> ".join(short(x) for x in c.split(" -> "))}' for c in cycles[:5])
    summary = (f'stack {stack} B (canary at {syms["_stack_canary"]:#x}); frames: ' + ', '.join(f'{re.sub(r"17h[0-9a-f]+E$", "", f)[-40:]} {s}' for f, s in top)) + extra
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
    # the call graph: main -> a -> b (+ a cycle b -> a); a tail call `b.w` counts; a branch inside a function does not
    g, ind = calls('\n'.join([
        'd0 <main>:', 'd0: bl\t0x20 <a>', 'd2: b\t0x6 <main+0x6>', 'd4: blx\tr3', 'd20 <a>:', 'd20: bl\t0x30 <b>',
        'd30 <b>:', 'd30: bl\t0x20 <a>', 'd32: b.w\t0x40 <c>', 'd34: bne.w\t0x50 <e>', 'd36: bx\tr2', 'd38: bx\tlr',
        'd40 <c>:', 'd40: bx lr', 'd50 <e>:', 'd50: ldr.w\tpc, [r0]']))
    assert g == {'main': {'a'}, 'a': {'b'}, 'b': {'a', 'c', 'e'}, 'c': set(), 'e': set()} and ind == {'main', 'b', 'e'}, (g, ind)
    # the cycle a <-> b with a big callee under b, reached by two roots (main, then x): the memo must not lose b -> big
    g2 = {'main': {'a'}, 'a': {'b'}, 'b': {'a', 'big'}, 'big': set(), 'x': {'b'}}
    fr2 = {'main': 10, 'a': 20, 'b': 30, 'big': 1000, 'x': 5}
    deep, path, cycles = chain(fr2, g2)
    assert deep == 10 + 20 + 30 + 1000 and path[-1] == 'big' and cycles == ['a -> b'], (deep, path, cycles)
    assert chain(fr2, {k: v for k, v in g2.items() if k != 'main'})[0] == 5 + 50 + 1000
    # a synthetic deep chain fails the check; a shallow one passes
    names = [f'f{i}' for i in range(10)]
    deepg = {n: {m} for n, m in zip(names, names[1:])} | {'f9': set()}
    deepfr = {n: 12_000 for n in names}  # 120,000 B in ten calls, each frame under its budget
    o, s = check(deepfr, syms, deepg)
    assert any('longest call chain' in x for x in o) and 'longest chain 120000 B' in s, (o, s)
    assert check({n: 100 for n in names}, syms, deepg)[0] == []
    # the ITCM check: a clean stop_itcm passes; a call, a branch out, a register call, an SDRAM literal fail; the
    # trampoline's jump and its SDRAM address are its job; code outside ITCM is not this check's
    isyms = {'_sitcm': 0x1000, '_eitcm': 0x1300}
    head = ['1000 <ram_trampoline>:', '1000: mov.w r2, #0xd0000000', '1028: bx\tr3', '102c: 50 ef 00 e0 \t.word\t0xd0000000',
            '1030 <_ZN13bryton_rustfw5board2hw9stop_itcm17h0123456789abcdefE>:', '1030: wfi',
            '1032: blo\t0x1080 <_ZN13bryton_rustfw5board2hw9stop_itcm17h0123456789abcdefE+0x50>', '1034: pop\t{r4, pc}']
    assert itcm_calls('\n'.join(head), isyms) == [], itcm_calls('\n'.join(head), isyms)
    head.append('1036: movt\tr2, #0xff80')   # a mask, not an address
    for bad in ('1038: bl\t0x1100 <x>', '1038: b.w\t0xd0001234 <memcpy>', '1038: blx\tr3', '1038: 00 10 00 d0 \t.word\t0xd0001000',
                '1038: movt\tr3, #0xd000', '1038: movt\tr3, #53248'):
        assert len(itcm_calls('\n'.join(head + [bad]), isyms)) == 1, bad
    assert itcm_calls('\n'.join(head[:4]), isyms) == ['hw.rs stop_itcm is not in ITCM (_sitcm.._eitcm)']
    assert itcm_calls('\n'.join(head + ['d0000000 <main>:', 'd0000000: bl\t0xd0001000 <x>']), isyms) == []
    print('stack_check.py selftest ok')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
        sys.exit()
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True, check=True).stdout.strip()
    bins = next(pathlib.Path(sysroot).glob('lib/rustlib/*/bin/llvm-objdump')).parent
    elf = pathlib.Path(sys.argv[1]) if sys.argv[1:] else ELF
    run = lambda tool, *a: subprocess.run([str(bins / tool), *a, str(elf)], capture_output=True, text=True, check=True).stdout
    dis = run('llvm-objdump', '-d', '--no-show-raw-insn')
    graph, indirect = calls(dis)
    syms = symbols(run('llvm-nm'))
    over, summary = check(frames(dis), syms, graph, indirect)
    over += itcm_calls(dis, syms)
    print(summary)
    if over:
        sys.exit('stack over budget:\n' + '\n'.join(over))
