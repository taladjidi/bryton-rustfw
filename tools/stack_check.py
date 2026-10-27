#!/usr/bin/env python3
"""The firmware's stack frames against their budgets (check.sh, after the build).

The stack is DTCM (128 KB less .data and .bss): on 2026-10-24 main's frame had grown to 111 KB (the app held there
several times over) and opening a workout overflowed it into .bss. This reads each function's frame from the
`sub sp, #n` of the release ELF (llvm-objdump from the toolchain) and fails when one is over its budget.
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
# frames allowed, bytes: the app's start (one temporary copy of App, once at boot), the course memory's (part of it
# built on the stack, once at boot, after the app's), main, anything else
BUDGET = {'make_app': 64_000, 'make_courses': 40_000, 'main': 16_384}
OTHER = 20_480

sysroot = subprocess.run(['rustc', '--print', 'sysroot'], capture_output=True, text=True, check=True).stdout.strip()
objdump = next(pathlib.Path(sysroot).glob('lib/rustlib/*/bin/llvm-objdump'))
text = subprocess.run([str(objdump), '-d', '--no-show-raw-insn', str(ELF)], capture_output=True, text=True).stdout
frames, fn = {}, None
for line in text.splitlines():
    m = re.match(r'^[0-9a-f]+ <(.*)>:$', line)
    if m:
        fn = m.group(1)
        continue
    m = re.search(r'\bsub(?:\.w|w)?\s+sp,\s*(?:sp,\s*)?#(0x[0-9a-f]+|\d+)', line)
    if m and fn:
        frames[fn] = max(frames.get(fn, 0), int(m.group(1), 0))
over = []
for f, size in sorted(frames.items(), key=lambda x: -x[1]):
    short = re.sub(r'^_ZN\d+bryton_rustfw\d+', '', f)
    budget = next((b for k, b in BUDGET.items() if re.search(rf'\b{k}\b|\d{k}\d|{k}17h', f)), OTHER)
    if size > budget:
        over.append(f'{size} B > {budget} B: {f}')
top = sorted(frames.items(), key=lambda x: -x[1])[:3]
print('stack frames: ' + ', '.join(f'{re.sub(r"17h[0-9a-f]+E$", "", f)[-40:]} {s}' for f, s in top))
if over:
    sys.exit('stack frames over budget:\n' + '\n'.join(over))
