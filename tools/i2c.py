#!/usr/bin/env python3
"""Talks to the sensor bus (I2C4, BOARD.md 14) from the computer, through the dev link's peek / poke: sets up I2C4
as the firmware does, then reads and writes chip registers. For exploring before (or beside) the firmware's driver.
Only volatile registers: none of these chips has non-volatile memory a register write could change.

    tools/i2c.py scan                       which 7-bit addresses acknowledge
    tools/i2c.py ids                        the ID registers of the chips stock knows (light, barometers, accel)
    tools/i2c.py read ADDR REG [N]          N bytes from REG on (hex or decimal)
    tools/i2c.py write ADDR REG BYTE...     bytes to REG on
    tools/i2c.py lux                        one light measurement (standby again after it)
    tools/i2c.py baro                       one barometer conversion: pressure and temperature

The firmware may use the bus between two of these transfers: run with a build that leaves I2C4 alone, or accept a
rare failed transfer. Each bus wait is a USB round trip; the bus holds SCL low meanwhile (clock stretching).
"""
import importlib.util, sys, time
from pathlib import Path

spec = importlib.util.spec_from_file_location('dev', Path(__file__).resolve().parent.parent / 'dev.py')
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)

GPIOD = 0x58020c00
MODER, OTYPER, AFRH = 0x00, 0x04, 0x24
RCC_D3CCIPR, RCC_APB4ENR, RCC_APB4RSTR = 0x58024458, 0x580244f4, 0x5802449c
I2C4 = 0x58001c00
CR1, CR2, TIMINGR, ISR, ICR, RXDR, TXDR = 0x00, 0x04, 0x10, 0x18, 0x1c, 0x24, 0x28
TXIS, RXNE, NACKF, STOPF, TC = 1 << 1, 1 << 2, 1 << 4, 1 << 5, 1 << 6
RD, START, AUTOEND = 1 << 10, 1 << 13, 1 << 25


class Bus:
    def __init__(self, s):
        self.s = s

    def peek(self, a):
        return int(dev.command(self.s, f'peek {a:#x}'), 16)

    def poke(self, a, v):
        assert dev.command(self.s, f'poke {a:#x} {v:#x}') == 'ok'

    def field(self, a, shift, bits, val):
        mask = ((1 << bits) - 1) << shift
        self.poke(a, (self.peek(a) & ~mask) | (val << shift))

    def up(self):
        self.field(RCC_D3CCIPR, 8, 2, 2)                                       # I2C4SEL = hsi_ker_ck
        self.poke(RCC_APB4ENR, self.peek(RCC_APB4ENR) | 1 << 7)               # I2C4EN
        for pin in (12, 13):                                                   # SCL, SDA: AF4 open drain
            self.field(GPIOD + OTYPER, pin, 1, 1)
            self.field(GPIOD + AFRH, 4 * (pin - 8), 4, 4)
            self.field(GPIOD + MODER, 2 * pin, 2, 2)
        if self.peek(I2C4 + CR1) & 1 == 0:
            self.poke(I2C4 + TIMINGR, 15 << 28 | 4 << 20 | 2 << 16 | 7 << 8 | 9)  # about 200 kHz (main.rs)
            self.poke(I2C4 + CR1, 1)

    def wait(self, flag):
        t0 = time.monotonic()
        while True:
            isr = self.peek(I2C4 + ISR)
            if isr & NACKF:
                self.poke(I2C4 + ICR, NACKF | STOPF)
                return False
            if isr & flag:
                return True
            if time.monotonic() - t0 > 0.5:
                self.poke(I2C4 + CR1, 0)                                       # reset the state machine
                self.poke(I2C4 + CR1, 1)
                raise SystemExit(f'I2C4 stuck, ISR {isr:#x}')

    def write(self, addr, reg, data=b''):
        self.poke(I2C4 + CR2, addr << 1 | (len(data) + 1) << 16 | AUTOEND | START)
        for b in bytes([reg]) + bytes(data):
            if not self.wait(TXIS):
                return False
            self.poke(I2C4 + TXDR, b)
        self.wait(STOPF)
        self.poke(I2C4 + ICR, STOPF)
        return True

    def read(self, addr, reg, n=1):
        self.poke(I2C4 + CR2, addr << 1 | 1 << 16 | START)
        if not self.wait(TXIS):
            return None
        self.poke(I2C4 + TXDR, reg)
        if not self.wait(TC):
            return None
        self.poke(I2C4 + CR2, addr << 1 | RD | n << 16 | AUTOEND | START)
        out = bytearray()
        for _ in range(n):
            if not self.wait(RXNE):
                return None
            out.append(self.peek(I2C4 + RXDR) & 0xff)
        self.wait(STOPF)
        self.poke(I2C4 + ICR, STOPF)
        return bytes(out)

    def acks(self, addr):
        """An empty write: acknowledged or not."""
        self.poke(I2C4 + CR2, addr << 1 | AUTOEND | START)
        self.wait(STOPF)
        nack = self.peek(I2C4 + ISR) & NACKF
        self.poke(I2C4 + ICR, NACKF | STOPF)
        return not nack


# (name, address, ID register, expected) as stock checks them (BOARD.md 14)
CHIPS = [('light LTR-308ALS', 0x53, 0x06, 0xb1), ('barometer LPS22HH', 0x5c, 0x0f, 0xb3),
         ('barometer BMP280', 0x76, 0xd0, 0x58), ('accelerometer LIS2DH12 class', 0x18, 0x0f, 0x33)]


def main():
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    num = lambda v: int(v, 0)
    with dev.connect() as s:
        bus = Bus(s)
        bus.up()
        cmd = args[0]
        if cmd == 'scan':
            print(' '.join(f'{a:#04x}' for a in range(0x08, 0x78) if bus.acks(a)))
        elif cmd == 'ids':
            for name, addr, reg, want in CHIPS:
                got = bus.read(addr, reg)
                state = 'no answer' if got is None else f'{got[0]:#04x}' + (' ok' if got[0] == want else f' (want {want:#04x})')
                print(f'{name:30} {addr:#04x} reg {reg:#04x}: {state}')
        elif cmd == 'read':
            got = bus.read(num(args[1]), num(args[2]), num(args[3]) if len(args) > 3 else 1)
            print('no answer' if got is None else got.hex(' '))
        elif cmd == 'write':
            print('ok' if bus.write(num(args[1]), num(args[2]), bytes(num(v) for v in args[3:])) else 'no answer')
        elif cmd == 'lux':
            print('before:', ' '.join(f'{r:#04x}={bus.read(0x53, r)[0]:#04x}' for r in (0x00, 0x04, 0x05, 0x07)))
            bus.write(0x53, 0x04, b'\x05')                                     # 20 bits / 400 ms (stock)
            bus.write(0x53, 0x05, b'\x02')                                     # gain 6 (stock)
            bus.write(0x53, 0x00, b'\x02')                                     # ALS_EN
            time.sleep(0.6)
            status = bus.read(0x53, 0x07)[0]
            d = bus.read(0x53, 0x0d, 3)
            bus.write(0x53, 0x00, b'\x00')                                     # standby
            raw = int.from_bytes(d, 'little') & 0xfffff
            print(f'status {status:#04x}, raw {raw}: {raw * 0.025:.1f} lux')
        elif cmd == 'baro':
            print('CTRL_REG1', bus.read(0x5c, 0x10).hex(), 'CTRL_REG2', bus.read(0x5c, 0x11).hex())
            bus.write(0x5c, 0x11, b'\x11')                                     # ONE_SHOT
            time.sleep(0.1)
            st = bus.read(0x5c, 0x27)[0]
            d = bus.read(0x5c, 0x28, 5)
            p = int.from_bytes(d[:3], 'little', signed=True)
            t = int.from_bytes(d[3:], 'little', signed=True)
            print(f'status {st:#04x}: {p / 4096:.2f} hPa, {t / 100:.2f} C')
        else:
            raise SystemExit(__doc__)


if __name__ == '__main__':
    main()
