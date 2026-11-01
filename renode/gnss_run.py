#!/usr/bin/env python3
"""The GNSS receiver on Renode: the firmware ELF boots on Renode's STM32H747, its SPI4 is renode/GnssSpi.cs, and that
talks to the host simulator's own receiver model (src/sim/gnss_model.rs, served by the ignored test src/sim/gnss_serve.rs)
in lockstep with Renode's virtual time. So `gnss_init`, `gnss_power` (PI6), `gnss_transfer` (PE4 by hand, 32-bit
TXDR / RXDR) and the driver on top run against the strict receiver, byte for byte. NOT part of check.sh (slow).
docs/devloop.md, "Renode with the GNSS model".

  renode/gnss_run.py [--seconds N] [--fault nak|stall|record] [--renode PATH]
  renode/gnss_run.py --selftest        the pipe protocol and the verdicts alone, no Renode (needs cargo)

Default: the receiver must be configured (a CFG-VALSET received and ACKed, none refused), NAV-PVT of the synthetic track
must reach the firmware about once a second, and the model must record no rule broken.
--fault nak: the receiver refuses the first CFG-VALSET (UBX-ACK-NAK); the firmware must send it once more (it reads the
answer: gps.rs `note_ack`) and get an ACK.
--fault stall: the receiver says nothing from 15 s to 17 s of virtual time (the model records the stream time-out, G2);
the firmware must keep reading and get NAV-PVT again afterwards.
--fault record: like the default but a rule broken is logged (viol, gap = longest unread gap in ms) instead of ending the run.
The log (target/renode/gnss_events.log) holds a line per change of the counts: ms, acks, naks, valsets, pvt, lost, viol.
Not proven: that the firmware decoded the fixes (no probe of its state yet: only the wire is seen), and anything of SPI timing.

Prints `skip:` and exits 0 without Renode or the release ELF.
"""
import argparse, os, re, signal, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
OUT = ROOT / 'target/renode'
RENODE = os.environ.get('RENODE', '/Applications/Renode.app/Contents/MacOS/renode')
STALL = (15000, 17000)                       # src/sim/gnss_serve.rs STALL_MS


def serve_exe():
    """The host test binary holding the receiver model (built here, `cargo test-host --no-run`)."""
    p = subprocess.run(['cargo', 'test-host', '--no-run'], cwd=ROOT, capture_output=True, text=True)
    m = re.search(r'Executable unittests src/lib\.rs \((.*)\)', p.stderr)
    if p.returncode or not m:
        sys.exit(f'cargo test-host --no-run failed:\n{p.stderr[-2000:]}')
    return str(ROOT / m.group(1))


def parse(path):
    """[(tag, ms, {name: int})] of the model's log."""
    out = []
    for l in Path(path).read_text().splitlines():
        t, ms, *kv = l.split()
        out.append((t, int(ms), {k: int(v) for k, v in (x.split('=') for x in kv)}))
    return out


def judge(fault, rows):
    """The failures (strings) of a run, from the log rows."""
    if not rows:
        return ['the model logged nothing']
    last = rows[-1][2]
    f = []
    if fault == 'nak':
        if last['naks'] != 1:
            f.append(f"expected one refused CFG-VALSET, naks={last['naks']}")
        if last['valsets'] < 2:
            f.append(f"the firmware did not send the refused CFG-VALSET again (valsets={last['valsets']})")
        if last['acks'] < 1:
            f.append('the second CFG-VALSET was not acknowledged')
    else:
        if last['naks'] or last['valsets'] < 1 or last['acks'] < 1:
            f.append(f"configuration not acknowledged: {last}")
    if fault == 'stall':
        if not last['lost']:
            f.append('the stall lost no epoch (the model did not see it)')
        before = [r[2]['pvt'] for r in rows if r[1] <= STALL[1]]
        if not before or last['pvt'] <= max(before):
            f.append('no NAV-PVT after the stall: the stream did not recover')
        if not any(r[1] > STALL[1] + 500 for r in rows):
            f.append('the firmware did not read the receiver after the stall')
    else:
        if last['lost'] or last['viol']:
            f.append(f"a rule broken: lost={last['lost']} viol={last['viol']}")
    if last['pvt'] < 3:
        f.append(f"only {last['pvt']} NAV-PVT frames reached the firmware")
    return f


def ubx(cls, ident, payload):
    b = bytes([cls, ident, len(payload) & 255, len(payload) >> 8]) + payload
    a = c = 0
    for x in b:
        a = (a + x) & 255
        c = (c + a) & 255
    return b'\xb5\x62' + b + bytes([a, c])


def selftest():
    """Drives the served model as GnssSpi.cs does: power, a CFG-VALSET, 25 s of 32-byte transfers every 50 ms."""
    import struct
    exe = serve_exe()
    for fault in ('', 'nak', 'stall'):
        OUT.mkdir(parents=True, exist_ok=True)
        log = OUT / 'gnss_selftest.log'
        env = dict(os.environ, GNSS_LOG=str(log), GNSS_FAULT=fault)
        p = subprocess.Popen([exe, 'gnss_serve', '--ignored', '--nocapture'], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        while not p.stdout.readline().startswith(b'GNSS-READY'):
            pass

        def rq(cmd, ms, extra=b'', n=1):
            p.stdin.write(cmd + struct.pack('<Q', ms) + extra)
            p.stdin.flush()
            return p.stdout.read(n)
        rq(b'P', 1500, b'\x01')
        valset = ubx(0x06, 0x8a, bytes([0, 1, 0, 0]) + struct.pack('<IB', 0x10230001, 1))   # CFG-ANA-USE_ANA, RAM
        tx = valset + b'\xff' * (-len(valset) % 32)
        sends = (1600, 1700) if fault == 'nak' else (1600,)    # a retrying firmware sends the refused one again
        for t in range(1600, 25000, 50):
            rq(b'C', t)
            for i in range(0, 32, 4):
                rq(b'W', t, tx[i:i + 4] if i < len(tx) and t in sends else b'\xff' * 4, 4)
            rq(b'E', t)
        p.stdin.write(b'Q')
        p.stdin.close()
        p.wait(timeout=20)
        fails = judge(fault or None, parse(log))
        print(f"selftest {fault or 'normal'}: {'ok' if not fails else fails}")
        if fails:
            sys.exit(1)
    print('gnss_run selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--seconds', type=float)
    ap.add_argument('--fault', choices=['nak', 'stall', 'record'])
    ap.add_argument('--selftest', action='store_true')
    ap.add_argument('--renode', default=RENODE)
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    # the firmware reads the receiver every 0.6 s until about 17 s of virtual time, then every 2.5 s (see the docs): a run
    # past that fails on the stream time-out unless it is the stall run, which expects it
    seconds = a.seconds or (24 if a.fault == 'stall' else 15)
    if not Path(a.renode).is_file():
        print(f'skip: gnss_run ({a.renode} missing)')
        return
    if not ELF.is_file():
        print(f'skip: gnss_run ({ELF} missing: cargo build --release)')
        return
    exe = serve_exe()
    for q in (exe, ROOT, ELF):
        assert '"' not in str(q), f'a quote in {q} would break the generated platform file'
    OUT.mkdir(parents=True, exist_ok=True)
    repl, log, events = OUT / 'gnss.repl', OUT / 'gnss.log', OUT / 'gnss_events.log'
    events.unlink(missing_ok=True)
    # SPI4 is replaced by the bridge (same address); PE4 (chip select) is its GPIO 0, PI6 (the supply) its GPIO 1
    repl.write_text(f'''gnss_spi: SPI.GnssSpi @ sysbus 0x40013400
    exe: "{exe}"
    args: "gnss_serve --ignored --nocapture"

gpioPortE:
    4 -> gnss_spi@0

gpioPortI:
    6 -> gnss_spi@1
''')
    script = [f'$elf=@{ELF}', f'include @{ROOT}/renode/s510.resc', 'logLevel 2', f'include @{ROOT}/renode/GnssSpi.cs',
              'sysbus Unregister sysbus.spi4', f'machine LoadPlatformDescription @{repl}',
              'gpioPortE OnGPIO 3 true', 'gpioPortA OnGPIO 1 true', f'emulation RunFor "{seconds}"', 'cpu PC', 'quit']
    limit = float(os.environ.get('RENODE_TIMEOUT') or 900)
    env = dict(os.environ, GNSS_LOG=str(events), GNSS_FAULT=a.fault or '')
    with open(log, 'w') as f:
        p = subprocess.Popen([a.renode, '--disable-gui', '--console', '-e', '; '.join(script)], stdout=f,
                             stderr=subprocess.STDOUT, cwd=ROOT, env=env, start_new_session=True)
        try:
            p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            sys.exit(f'renode: timed out after {limit:g} s (RENODE_TIMEOUT); log {log}')
        if p.returncode:
            sys.exit(f'renode exited {p.returncode}; log {log}')
    text = re.sub(r'\x1b\[[0-9;]*m', '', log.read_text(errors='replace'))
    fails = []
    if 'gnss harness gone' in text:
        fails.append('the harness died (a rule broken outside the expected ones panics the model): ' +
                     '; '.join(re.findall(r'gnss harness gone: (.*)', text)[:2]))
    if not events.is_file():
        fails.append('the model wrote no log: the receiver was never powered or read')
    else:
        rows = parse(events)
        print(f"{len(rows)} log lines, last: {rows[-1] if rows else None}")
        fails += judge(a.fault, rows)
    for f in fails:
        print('FAIL', f)
    print('gnss_run ok' if not fails else f'{len(fails)} failures (log: {log})')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
