#!/usr/bin/env python3
"""The whole radio path on Renode: the firmware ELF boots on Renode's STM32H747, its USART2 is renode/NrfUsart.cs, and
that talks to Bryton's real nRF52 app (tools/nrf_emu.py --serve, Unicorn) in lockstep with Renode's virtual time.
So `nrf_reset`, `nrf_send`, `nrf_recv` and the USART2 setup of src/main.rs, and `nrf::start`, run against the real chip app
byte for byte. NOT part of check.sh (slow). docs/devloop.md, "Renode with the radio chip's real app".

  renode/nrf_run.py [--seconds 6] [--hang N] [--no-drain] [--renode PATH]

Default: the radio start must be answered by the real app (version, cfg_set x2, ble enable, each with status 0), and
the app must still run 30 s of virtual time later (the phone link setup, advertising and a scan call only SVCs the stub knows).
--hang N: after the N-th command since the chip's reset one byte too many goes on the wire (a command one byte too
long); the app must stop, and the firmware's radio watchdog (src/app/nrf_watch.rs) must notice and reset the chip through
PA6 (a second reset, then the commands answered again). The run is long in virtual
time (the first recovery waits), so --seconds defaults to 70 with --hang (about 3.5 minutes of wall time; the plain run is 6 s, 25 s of wall).

--no-drain: a test of the overrun check. The bridge's receive FIFO is 4 bytes, the firmware's reads never empty it and RTS
is ignored, so the chip's first answer overruns it; the run passes only when `nrf overrun` is logged. The other runs fail on one.

Prints `skip:` and exits 0 without Renode, python `unicorn`, the app binary (NRF_BIN) or the release ELF.
"""
import argparse, os, re, shutil, signal, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
OUT = ROOT / 'target/renode'
RENODE = os.environ.get('RENODE', '/Applications/Renode.app/Contents/MacOS/renode')
NRF_BIN = os.environ.get('NRF_BIN', str(Path.home() / 'Documents/bryton/firmware/R039/nordic.bin'))


def skip(why):
    print(f'skip: nrf_run ({why})')
    sys.exit(0)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--seconds', type=float)
    ap.add_argument('--hang', type=int, default=0)
    ap.add_argument('--no-drain', action='store_true')
    ap.add_argument('--renode', default=RENODE)
    a = ap.parse_args()
    seconds = a.seconds or (70 if a.hang else 35)
    py = os.environ.get('NRF_PYTHON') or shutil.which('python3') or 'python3'
    if not Path(a.renode).is_file():
        skip(f'{a.renode} missing')
    if not Path(NRF_BIN).is_file():
        skip(f'{NRF_BIN} missing')
    if subprocess.run([py, '-I', '-c', 'import unicorn'], capture_output=True).returncode:
        skip('python unicorn missing')
    if not ELF.is_file():
        skip(f'{ELF} missing: cargo build --release')

    for q in (py, ROOT, ELF):
        assert '"' not in str(q), f'a quote in {q} would break the generated platform file'
    OUT.mkdir(parents=True, exist_ok=True)
    repl, log = OUT / 'nrf.repl', OUT / 'nrf.log'
    # USART2 is replaced by the bridge (the registration is the same address); PA6, the chip's reset, drives its GPIO 0
    repl.write_text(f'''nrf_usart: UART.NrfUsart @ sysbus 0x40004400
    python: "{py}"
    tool: "{ROOT}/tools/nrf_emu.py"
    injectAfterFrames: {a.hang}
    noDrain: {'true' if a.no_drain else 'false'}
    IRQ -> nvic@38

gpioPortA:
    6 -> nrf_usart@0
''')
    script = [f'$elf=@{ELF}', f'include @{ROOT}/renode/s510.resc', 'logLevel 2', f'include @{ROOT}/renode/NrfUsart.cs',
              'sysbus Unregister sysbus.usart2', f'machine LoadPlatformDescription @{repl}',
              'gpioPortE OnGPIO 3 true', 'gpioPortA OnGPIO 1 true', f'emulation RunFor "{seconds}"', 'cpu PC', 'quit']
    limit = float(os.environ.get('RENODE_TIMEOUT') or 600)
    with open(log, 'w') as f:
        # its own process group: on a time-out the group (Renode and the python child) is killed
        p = subprocess.Popen([a.renode, '--disable-gui', '--console', '-e', '; '.join(script)], stdout=f,
                             stderr=subprocess.STDOUT, cwd=ROOT, start_new_session=True)
        try:
            p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            sys.exit(f'renode: timed out after {limit:g} s (RENODE_TIMEOUT); log {log}')
        if p.returncode:
            sys.exit(f'renode exited {p.returncode}; log {log}')
    text = re.sub(r'\x1b\[[0-9;]*m', '', log.read_text(errors='replace'))
    lines = [l.split('] ', 1)[-1] if '] ' in l else l for l in text.splitlines() if 'nrf' in l or 'rror' in l]
    tx = [m.groups() for m in re.finditer(r'nrf tx frame (\d+): (\w\w) (\w\w)', text)]
    rx = [m.groups() for m in re.finditer(r'nrf rx frame: type (\w\w) op (\w\w) status (\w{8})', text)]
    resets = len(re.findall(r'nrf: chip reset #', text))
    stopped = re.findall(r'chip app stopped: (.*?) at (\d+) us', text)
    print(f'{len(tx)} commands, {len(rx)} frames from the chip, {resets} chip resets, stopped: {stopped}')
    for l in lines[:40]:
        print('  ', l)
    fails = []
    if 'nrf harness gone' in text:
        fails.append('the harness died')
    if 'nrf baud error' in text:
        fails.append('BRR and the kernel clock do not give 1 Mbaud')
    overrun = 'nrf overrun' in text
    if overrun != a.no_drain:
        fails.append('an overrun' if overrun else 'no-drain produced no overrun')
    answered = [op for t, op, st in rx if t == '01' and st == '00000000']
    if a.no_drain:
        pass
    elif not a.hang:
        for op in ('69', '60', '65'):      # cfg_set, ble enable, version get (src/drivers/nrf.rs `start`)
            if op not in answered:
                fails.append(f'no status 0 answer to command 0x{op}')
        if stopped:
            fails.append(f'the chip app stopped: {stopped}')
    else:
        if not stopped:
            fails.append('one byte too many did not stop the app')
        if resets < 2:
            fails.append('the watchdog did not reset the chip through PA6')
        # only the frames after the last chip reset count: a status 0 answer to cfg_set or version get
        after = re.findall(r'nrf rx frame: type (\w\w) op (\w\w) status (\w{8})', text.rsplit('nrf: chip reset #', 1)[-1])
        if not [1 for t, op, st in after if t == '01' and op in ('69', '65') and st == '00000000']:
            fails.append('nothing answered with status 0 after the last reset')
    for f in fails:
        print('FAIL', f)
    print('nrf_run ok' if not fails else f'{len(fails)} failures (log: {log})')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
