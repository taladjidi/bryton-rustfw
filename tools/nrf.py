#!/usr/bin/env python3
"""Talks to the Nordic BLE / ANT chip from the computer, through the dev link's peek / poke (BOARD.md 11): sets
up USART2 and the pins as stock does, resets the chip, and exchanges nRF5 serialization packets byte by byte.
For exploring the protocol before the firmware has a driver; nothing is written to the eMMC or to the Nordic
chip's flash.

    tools/nrf.py version            reset, sd_ble_enable, sd_ble_version_get
    tools/nrf.py scan [seconds]     reset, enable, active BLE scan; lists advertisers (address, RSSI, name)
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
        self.poke(0x58024454, self.peek(0x58024454) & ~7 | 3)                  # USART2 kernel clock: HSI 64 MHz
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
        self.poke(USART2 + BRR, 64)                                            # 64 MHz / 1 Mbaud, as main.rs
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


def packets(data):
    """Splits received bytes into payloads (2-byte LE length + payload)."""
    out, i = [], 0
    while i + 2 <= len(data):
        n = int.from_bytes(data[i:i + 2], 'little')
        out.append(data[i + 2:i + 2 + n])
        i += 2 + n
    return out


def adv_report(p):
    """BLE_GAP_EVT_ADV_REPORT (API v5): (address, rssi, scan_rsp, name or None) or None."""
    if len(p) < 23 or p[0] != 2 or p[1:3] != b'\x1d\x00':
        return None
    addr = ':'.join(f'{b:02x}' for b in reversed(p[6:12]))
    rssi = p[19] - 256 if p[19] > 127 else p[19]
    data, name, i = p[23:23 + p[21]], None, 0
    while i + 1 < len(data) and data[i]:
        if data[i + 1] in (8, 9):
            name = data[i + 2:i + 1 + data[i]].decode(errors='replace')
        i += 1 + data[i]
    return addr, rssi, p[20] & 1, name


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ('version', 'send', 'scan'):
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
        elif sys.argv[1] == 'scan':
            secs = float(sys.argv[2]) if len(sys.argv) > 2 else 5
            n.reset()
            print('sd_ble_enable:', n.command(bytes([0x00, 0x60])).hex(' '))
            # sd_ble_gap_scan_start, params present: active, interval = window = 60 ms (0x60 x 0.625), no timeout
            got = n.command(bytes([0x00, 0x8a, 1, 1, 0x60, 0, 0x60, 0, 0, 0]), wait=secs)
            got += n.command(bytes([0x00, 0x8b]), wait=0.3)                    # scan_stop
            replies = [p.hex(' ') for p in packets(got) if p[:1] == b'\x01']
            print('scan_start, scan_stop replies:', replies)
            seen = {}
            for p in packets(got):
                r = adv_report(p)
                if r:
                    addr, rssi, _, name = r
                    old = seen.get(addr, (rssi, None, 0))
                    seen[addr] = (max(rssi, old[0]), name or old[1], old[2] + 1)
            print(f'{len(got)} bytes, {len(seen)} advertisers in {secs:g} s:')
            for addr, (rssi, name, count) in sorted(seen.items(), key=lambda kv: -kv[1][0]):
                print(f'  {addr}  {rssi:4d} dBm  {count:3d} reports  {name or ""}')
        else:
            print(n.send(bytes.fromhex(sys.argv[2])).hex(' '))


if __name__ == '__main__':
    main()
