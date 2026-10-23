#!/usr/bin/env python3
"""Talks to the Nordic BLE / ANT chip from the computer, through the dev link's peek / poke (BOARD.md 11): sets
up USART2 and the pins as stock does, resets the chip, and exchanges nRF5 serialization packets byte by byte.
For exploring the protocol before the firmware has a driver; nothing is written to the eMMC or to the Nordic
chip's flash.

    tools/nrf.py version            reset, sd_ble_enable, sd_ble_version_get
    tools/nrf.py send 0300006501    one raw packet (length header included), print the reply

Never send DFU or flash commands: the Nordic firmware is not ours to replace (PI4 low at reset is its bootloader).
"""
import importlib.util, sys, time
from pathlib import Path

spec = importlib.util.spec_from_file_location('dev', Path(__file__).resolve().parent.parent / 'dev.py')
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)

GPIOA, GPIOD, GPIOI = 0x58020000, 0x58020c00, 0x58022000
MODER, PUPDR, IDR, BSRR, AFRL = 0x00, 0x0c, 0x10, 0x18, 0x20
USART2 = 0x40004400
CR1, CR3, BRR, ISR, ICR, RDR, TDR = 0x00, 0x08, 0x0c, 0x1c, 0x20, 0x24, 0x28
RXFNE, TXFNF = 1 << 5, 1 << 7


class Nrf:
    def __init__(self, s):
        self.s = s

    def peek(self, a):
        return int(dev.command(self.s, f'peek {a:#x}'), 16)

    def poke(self, a, v):
        assert dev.command(self.s, f'poke {a:#x} {v:#x}') == 'ok'

    def field(self, base, reg, pin, bits, val):
        mask = ((1 << bits) - 1) << (pin * bits)
        self.poke(base + reg, (self.peek(base + reg) & ~mask) | (val << (pin * bits)))

    def reset(self):
        """Stock's sequence: PI4 high (kept), PA6 low while USART2 comes up, release, 800 ms."""
        self.poke(0x580244e8, self.peek(0x580244e8) | 1 << 17)                # RCC_APB1LENR.USART2EN
        for pin in (3, 4, 5, 6):                                               # PD3 CTS, PD4 RTS, PD5 TX, PD6 RX
            self.field(GPIOD, PUPDR, pin, 2, 1)
            self.field(GPIOD, AFRL, pin, 4, 7)
            self.field(GPIOD, MODER, pin, 2, 2)
        self.poke(GPIOI + BSRR, 1 << 4)                                        # PI4 high: application, not DFU
        self.field(GPIOI, PUPDR, 4, 2, 1)
        self.field(GPIOI, MODER, 4, 2, 1)
        self.poke(GPIOA + BSRR, 1 << (16 + 6))                                 # PA6 low: reset held
        self.field(GPIOA, PUPDR, 6, 2, 1)
        self.field(GPIOA, MODER, 6, 2, 1)
        self.poke(USART2 + CR1, 0)
        self.poke(USART2 + BRR, 48)                                            # PLL3Q 48 MHz / 1 Mbaud
        self.poke(USART2 + CR3, 0x300)                                         # RTSE, CTSE
        self.poke(USART2 + CR1, 1 << 29 | 0xd)                                 # FIFOEN, TE, RE, UE
        time.sleep(0.15)
        self.poke(GPIOA + BSRR, 1 << 6)                                        # release
        time.sleep(0.8)
        while self.peek(USART2 + ISR) & RXFNE:                                 # a glitch byte at the release
            self.peek(USART2 + RDR)
        self.poke(USART2 + ICR, 0xf)

    def send(self, packet, wait=2.0):
        """Sends one packet (2-byte length header included) and returns what arrives within `wait` seconds."""
        for b in packet:
            while not self.peek(USART2 + ISR) & TXFNF:
                pass
            self.poke(USART2 + TDR, b)
        got, t0 = bytearray(), time.monotonic()
        while time.monotonic() - t0 < wait:
            if self.peek(USART2 + ISR) & RXFNE:
                got.append(self.peek(USART2 + RDR) & 0xff)
        return bytes(got)

    def command(self, payload, wait=2.0):
        """A serialization payload ([type][op][args]) with its length header added."""
        return self.send(len(payload).to_bytes(2, 'little') + payload, wait)


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ('version', 'send'):
        sys.exit(__doc__)
    with dev.connect() as s:
        n = Nrf(s)
        if sys.argv[1] == 'version':
            n.reset()
            enable = n.command(bytes([0x00, 0x60]))                            # sd_ble_enable
            ver = n.command(bytes([0x00, 0x65, 0x01]))                         # sd_ble_version_get
            print(f'sd_ble_enable: {enable.hex(" ")}')
            print(f'sd_ble_version_get: {ver.hex(" ")}')
            if len(ver) == 13 and ver[2:8] == bytes([0x01, 0x65, 0, 0, 0, 0]):
                image, boot, sd = ver[8], int.from_bytes(ver[9:11], 'little'), int.from_bytes(ver[11:13], 'little')
                print(f'Nordic image {image}, bootloader {boot}, SD {sd} (the fields as Bryton fills them)')
        else:
            print(n.send(bytes.fromhex(sys.argv[2])).hex(' '))


if __name__ == '__main__':
    main()
