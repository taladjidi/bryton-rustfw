#!/usr/bin/env python3
"""Talks to the u-blox GNSS receiver from the computer, through the dev link's peek / poke (BOARD.md 13): powers
it as stock does (PI6 high), sets up SPI4, and exchanges bytes. For exploring before the firmware has a driver;
nothing is written to the receiver's flash (no UBX-CFG with a flash layer, no firmware update).

    tools/gnss.py dump [seconds]      power on, print what the receiver sends (NMEA lines, UBX frames)
    tools/gnss.py ver                 power on, poll UBX-MON-VER (receiver and firmware versions)
    tools/gnss.py off                 power off (PI6 low)

SPI4 here runs on the HSI (64 MHz / 64 = 1 MHz, stock's rate), software chip select on PE4. Each 4 bytes cost a
few USB round trips: slow, but enough for 1 Hz output.
"""
import importlib.util, sys, time
from pathlib import Path

spec = importlib.util.spec_from_file_location('dev', Path(__file__).resolve().parent.parent / 'dev.py')
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)

GPIOE, GPIOI = 0x58021000, 0x58022000
MODER, PUPDR, BSRR, AFRL = 0x00, 0x0c, 0x18, 0x20
RCC_D2CCIP1R, RCC_APB2ENR, RCC_APB2RSTR = 0x58024450, 0x580244f0, 0x58024498
SPI4 = 0x40013400
CR1, CR2, CFG1, CFG2, SR, IFCR, TXDR, RXDR = 0x00, 0x04, 0x08, 0x0c, 0x14, 0x18, 0x20, 0x30
RXP, TXP = 1 << 0, 1 << 1


class Gnss:
    def __init__(self, s):
        self.s = s

    def peek(self, a):
        return int(dev.command(self.s, f'peek {a:#x}'), 16)

    def poke(self, a, v):
        assert dev.command(self.s, f'poke {a:#x} {v:#x}') == 'ok'

    def field(self, base, reg, pin, bits, val):
        mask = ((1 << bits) - 1) << (pin * bits)
        self.poke(base + reg, (self.peek(base + reg) & ~mask) | (val << (pin * bits)))

    def power(self, on):
        self.poke(GPIOI + BSRR, 1 << 6 if on else 1 << (16 + 6))
        self.field(GPIOI, MODER, 6, 2, 1)                                      # PI6 output

    def spi_up(self):
        self.poke(RCC_APB2ENR, self.peek(RCC_APB2ENR) | 1 << 13)              # SPI4EN
        self.poke(RCC_D2CCIP1R, self.peek(RCC_D2CCIP1R) & ~(7 << 16) | 3 << 16)   # SPI45SEL = hsi_ker_ck
        for pin in (2, 5, 6):                                                  # SCK, MISO, MOSI: AF5
            self.field(GPIOE, AFRL, pin, 4, 5)
            self.field(GPIOE, MODER, pin, 2, 2)
        self.field(GPIOE, PUPDR, 2, 2, 2)                                      # SCK pull-down, as stock
        self.poke(GPIOE + BSRR, 1 << 4)                                        # PE4 = chip select, high
        self.field(GPIOE, MODER, 4, 2, 1)
        self.poke(RCC_APB2RSTR, 1 << 13)                                       # SPI4 from reset: a rerun finds it
        self.poke(RCC_APB2RSTR, 0)                                             # enabled, with MODF latched
        self.poke(SPI4 + CFG1, 5 << 28 | 3 << 5 | 7)                          # /64, FIFO threshold 4, 8 bit
        self.poke(SPI4 + CR1, 1 << 12)                                         # SSI before MASTER (BOARD.md 6)
        self.poke(SPI4 + CFG2, 1 << 31 | 1 << 26 | 1 << 22)                   # AFCNTR, SSM, MASTER; mode 0
        self.poke(SPI4 + CR2, 0)                                               # TSIZE 0: endless
        self.poke(SPI4 + CR1, 1 << 12 | 1)                                     # SPE
        self.poke(SPI4 + CR1, 1 << 12 | 1 << 9 | 1)                            # CSTART

    def xfer4(self, out=b'\xff\xff\xff\xff'):
        """Four bytes out, four bytes in (32-bit FIFO access packs four 8-bit frames)."""
        self.poke(SPI4 + TXDR, int.from_bytes(out, 'little'))
        t0 = time.monotonic()
        while not self.peek(SPI4 + SR) & RXP:
            if time.monotonic() - t0 > 0.5:
                raise SystemExit(f'SPI4 stuck, SR {self.peek(SPI4 + SR):#x}')
        return self.peek(SPI4 + RXDR).to_bytes(4, 'little')

    def transfer(self, out):
        self.poke(GPIOE + BSRR, 1 << (16 + 4))                                 # CS low
        out = out + b'\xff' * (-len(out) % 4)
        got = b''.join(self.xfer4(out[i:i + 4]) for i in range(0, len(out), 4))
        self.poke(GPIOE + BSRR, 1 << 4)                                        # CS high
        return got

    def read(self, n):
        return self.transfer(b'\xff' * n)


def ubx(cls, mid, payload=b''):
    body = bytes([cls, mid]) + len(payload).to_bytes(2, 'little') + payload
    a = b = 0
    for x in body:
        a = (a + x) & 0xff
        b = (b + a) & 0xff
    return b'\xb5\x62' + body + bytes([a, b])


def show(data):
    """Prints NMEA lines and UBX frames found in `data` (0xff is the idle fill)."""
    i = 0
    while i < len(data):
        if data[i] == 0xff:
            i += 1
        elif data[i:i + 2] == b'\xb5\x62' and i + 6 <= len(data):
            n = int.from_bytes(data[i + 4:i + 6], 'little')
            frame = data[i:i + 8 + n]
            print(f'UBX {frame[2]:02x} {frame[3]:02x} len {n}: {frame[6:6 + n].hex(" ")}')
            i += 8 + n
        else:
            j = data.find(b'\n', i)
            j = len(data) if j < 0 else j + 1
            print(data[i:j].decode(errors='replace').rstrip())
            i = j


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ('dump', 'ver', 'off'):
        sys.exit(__doc__)
    with dev.connect() as s:
        g = Gnss(s)
        if sys.argv[1] == 'off':
            return g.power(False)
        g.spi_up()
        if g.peek(GPIOI + 0x14) & 1 << 6 == 0 or g.peek(GPIOI + MODER) >> 12 & 3 != 1:
            g.power(True)
            print('powered on, waiting 3 s (stock does)')
            time.sleep(3)
        if sys.argv[1] == 'ver':
            g.transfer(ubx(0x0a, 0x04))                                        # UBX-MON-VER poll
            data = b''.join(g.read(200) for _ in range(5))
            show(data)
            for i in range(len(data)):
                if data[i:i + 4] == b'\xb5\x62\x0a\x04':
                    n = int.from_bytes(data[i + 4:i + 6], 'little')
                    p = data[i + 6:i + 6 + n]
                    print('software:', p[:30].rstrip(b'\0').decode(), '| hardware:', p[30:40].rstrip(b'\0').decode())
                    for k in range(40, n, 30):
                        print('  ', p[k:k + 30].rstrip(b'\0').decode())
        else:
            secs = float(sys.argv[2]) if len(sys.argv) > 2 else 3
            data, t0 = b'', time.monotonic()
            while time.monotonic() - t0 < secs:
                data += g.read(200)
            print(f'{len(data)} bytes, {sum(b != 0xff for b in data)} not idle')
            show(data)


if __name__ == '__main__':
    main()
